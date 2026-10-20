//! 图片文字识别线程（低优先级）：新复制的图片与“重新识别”插队并立即识别，历史图片回填排队尾；
//! 面板可见期间只暂停回填，正在识别的那张不打断。
//! 日志只记 id、状态、耗时与计数，不记文字与路径。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use image::ImageReader;
use tauri::{AppHandle, Manager};
use xpaste_core::ocr_state::{EMPTY, FAILED, PENDING, SKIPPED, TEXT, UNSUPPORTED};
use xpaste_platform::OcrError;

use crate::events::{RECORD_UPDATED, emit_summary};
use crate::state::AppState;

/// 启动后多久开始回填历史图片
pub const BACKFILL_DELAY: Duration = Duration::from_secs(30);
/// 回填项之间的间隔，避免启动即满负荷
const BACKFILL_GAP: Duration = Duration::from_millis(500);
const BACKFILL_LIMIT: usize = 5000;
const MAX_BYTES: u64 = 20 * 1024 * 1024;
const MAX_SIDE: u32 = 8000;
/// 交给引擎前缩到的长边
const OCR_SIDE: u32 = 2000;
const MIN_CONFIDENCE: f32 = 0.3;
/// 软超时：只记日志。macOS 每个进程首次调用要加载模型（约 8 s），慢但成功的结果照常记录
const SLOW: Duration = Duration::from_secs(10);

struct Item {
    id: i64,
    /// 回填项：面板可见时等待；其余（新复制、重新识别）照常取出
    backfill: bool,
}

/// 待识别的 id。纯逻辑，不碰库与线程。
#[derive(Default)]
pub struct Queue {
    items: VecDeque<Item>,
    paused: bool,
}

impl Queue {
    /// 插队；已在队中的移到队首并改为非回填项。
    pub fn push_front(&mut self, id: i64) {
        self.items.retain(|it| it.id != id);
        self.items.push_front(Item {
            id,
            backfill: false,
        });
    }

    /// 回填排队尾；已在队中的不动。
    pub fn push_back(&mut self, id: i64) {
        if !self.items.iter().any(|it| it.id == id) {
            self.items.push_back(Item { id, backfill: true });
        }
    }

    pub fn set_paused(&mut self, on: bool) {
        self.paused = on;
    }

    /// `(id, 是否回填项)`。暂停时只取非回填项，回填项保留。
    pub fn next(&mut self) -> Option<(i64, bool)> {
        let i = if self.paused {
            self.items.iter().position(|it| !it.backfill)?
        } else {
            0
        };
        self.items.remove(i).map(|it| (it.id, it.backfill))
    }

    pub fn drain(&mut self) -> Vec<i64> {
        self.items.drain(..).map(|it| it.id).collect()
    }
}

/// 存在 `AppState` 里的队列句柄；`supported` 为启动时探测一次的引擎可用性。
#[derive(Clone)]
pub struct Handle {
    inner: Arc<(Mutex<Queue>, Condvar)>,
    pub supported: bool,
}

impl Handle {
    pub fn new(supported: bool) -> Self {
        Self {
            inner: Arc::default(),
            supported,
        }
    }

    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.inner.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 新复制的图片与重新识别：插队，面板可见也立即开始。
    pub fn push_front(&self, id: i64) {
        self.queue().push_front(id);
        self.inner.1.notify_one();
    }

    pub fn push_back_many(&self, ids: &[i64]) {
        let mut q = self.queue();
        ids.iter().for_each(|&id| q.push_back(id));
        drop(q);
        self.inner.1.notify_one();
    }

    /// 面板显示时暂停回填、隐藏时继续；正在识别的那张不打断。
    pub fn set_paused(&self, on: bool) {
        self.queue().set_paused(on);
        self.inner.1.notify_one();
    }

    fn wait_next(&self) -> (i64, bool) {
        let mut q = self.queue();
        loop {
            if let Some(item) = q.next() {
                return item;
            }
            q = self.inner.1.wait(q).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// 设置关闭识别：队列里还没识别的置为跳过（已识别文字保留）。
    pub fn disable(&self, app: &AppHandle) {
        let ids = self.queue().drain();
        let state = app.state::<AppState>();
        // 一条 UPDATE；先绑定结果，store 锁在本语句末释放，emit 不在锁内
        let skipped = state.store().set_ocr_many(&ids, SKIPPED);
        let skipped = skipped.unwrap_or_else(|e| {
            log::warn!("ocr 置为跳过失败：{e}");
            Vec::new()
        });
        log::info!("ocr 已关闭，跳过 {} 张", skipped.len());
        skipped
            .into_iter()
            .for_each(|id| emit_summary(app, RECORD_UPDATED, id));
    }

    /// `delay` 后若开着识别与回填，把待识别、已跳过（引擎可用时含上次运行“不支持”）的历史图片排到队尾。
    /// 失败的不自动重试。
    pub fn schedule_backfill(&self, app: &AppHandle, delay: Duration) {
        let (h, app) = (self.clone(), app.clone());
        let spawned = std::thread::Builder::new()
            .name("ocr-backfill".into())
            .spawn(move || {
                std::thread::sleep(delay);
                let state = app.state::<AppState>();
                let on = {
                    let s = state.settings();
                    s.ocr_enabled && s.ocr_backfill
                };
                if !on {
                    return;
                }
                let states: &[u8] = if h.supported {
                    &[PENDING, SKIPPED, UNSUPPORTED]
                } else {
                    &[PENDING, SKIPPED]
                };
                let ids = state.store().ocr_pending_ids(states, BACKFILL_LIMIT);
                match ids {
                    Ok(ids) => {
                        log::info!("ocr 回填 {} 张", ids.len());
                        h.push_back_many(&ids);
                    }
                    Err(e) => log::warn!("ocr 读取回填列表失败：{e}"),
                }
            });
        if let Err(e) = spawned {
            log::warn!("ocr 回填线程启动失败：{e}");
        }
    }
}

/// 探测引擎并启动识别线程。线程只在 `AppState` 托管后才会收到任务（句柄存在 `AppState` 里）。
pub fn start(app: AppHandle) -> Handle {
    let h = Handle::new(xpaste_platform::ocr_available());
    log::info!("ocr 引擎可用={}", h.supported);
    let worker = h.clone();
    let spawned = std::thread::Builder::new()
        .name("ocr".into())
        .spawn(move || {
            xpaste_platform::lower_thread_priority();
            loop {
                let (id, backfill) = worker.wait_next();
                // 回填中途关掉“补识别”：剩余回填项原样留着，重新打开时再入队
                if backfill && !app.state::<AppState>().settings().ocr_backfill {
                    continue;
                }
                if process(&app, worker.supported, id) && backfill {
                    std::thread::sleep(BACKFILL_GAP);
                }
            }
        });
    if let Err(e) = spawned {
        log::warn!("ocr 线程启动失败：{e}");
    }
    h
}

/// 处理一张；返回是否调用了识别引擎或改了状态（回填据此歇 500 ms，引擎不可用时成批改“不支持”也不刷屏）。
fn process(app: &AppHandle, supported: bool, id: i64) -> bool {
    let state = app.state::<AppState>();
    // 只处理待识别、已跳过、本机不支持的：重复入队或已被处理、已删除的直接丢弃
    let prior = state.store().ocr_state(id);
    let prior = match prior {
        Ok(Some(s @ (PENDING | SKIPPED | UNSUPPORTED))) => s,
        Ok(_) => {
            log::debug!("ocr id={id} 无需识别");
            return false;
        }
        Err(e) => {
            log::warn!("ocr id={id} 读取状态失败：{e}");
            return false;
        }
    };
    let enabled = state.settings().ocr_enabled;
    let t0 = Instant::now();
    let (k, text, ran) = match (enabled, supported) {
        (false, _) => (SKIPPED, None, false),
        (true, false) => (UNSUPPORTED, None, false),
        (true, true) => recognize(&state, id),
    };
    let ms = t0.elapsed().as_millis();
    log::info!("ocr id={id} state={k} ms={ms}");
    if k == prior {
        return ran;
    }
    // 记录在识别期间被删除时 set_ocr 返回 false，结果丢弃。先绑定再 match：store 锁在本语句末释放，emit 不在锁内
    let written = state.store().set_ocr(id, k, text.as_deref());
    match written {
        Ok(true) => emit_summary(app, RECORD_UPDATED, id),
        Ok(false) => {}
        Err(e) => log::warn!("ocr id={id} 写入失败：{e}"),
    }
    true
}

/// 按内容识别格式读尺寸（只读文件头），不信扩展名。
fn dimensions(path: &Path) -> Option<(u32, u32)> {
    ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .ok()?
        .into_dimensions()
        .ok()
}

/// 位图记录识别 `images/<id>.png`；图片文件引用识别源文件。
fn source(state: &AppState, id: i64) -> Option<PathBuf> {
    let bitmap = state.data_dir.join("images").join(format!("{id}.png"));
    if bitmap.exists() {
        return Some(bitmap);
    }
    let d = state.store().detail(id).ok()??;
    d.files
        .into_iter()
        .next()
        .filter(|f| f.exists)
        .map(|f| f.path.into())
}

/// `(状态, 文字, 是否调用了引擎)`
fn recognize(state: &AppState, id: i64) -> (u8, Option<String>, bool) {
    let Some(path) = source(state, id) else {
        return (FAILED, None, false);
    };
    let too_big = std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES)
        || dimensions(&path).is_some_and(|(w, h)| w.max(h) > MAX_SIDE);
    if too_big {
        return (SKIPPED, None, false);
    }
    let t0 = Instant::now();
    let r = xpaste_platform::ocr_png(&path, OCR_SIDE);
    if t0.elapsed() > SLOW {
        log::warn!("ocr id={id} 超过 {} s", SLOW.as_secs());
    }
    let (k, text) = match r {
        Ok(Some(t)) if !t.text.trim().is_empty() && t.confidence >= MIN_CONFIDENCE => {
            (TEXT, Some(t.text))
        }
        Ok(Some(_)) => (EMPTY, None),
        Ok(None) => (UNSUPPORTED, None),
        Err(e) => {
            // 只记错误类别：引擎描述不进日志
            let kind = match e {
                OcrError::Decode => "decode",
                OcrError::Timeout => "timeout",
                OcrError::Engine(_) => "engine",
            };
            log::info!("ocr id={id} 失败 {kind}");
            (FAILED, None)
        }
    };
    (k, text, true)
}

#[cfg(test)]
mod tests {
    use super::Queue;

    #[test]
    fn queue_pauses_while_visible() {
        let mut q = Queue::default();
        q.push_back(1);
        q.set_paused(true);
        assert_eq!(q.next(), None);
        q.set_paused(false);
        assert_eq!(q.next(), Some((1, true)));
    }

    #[test]
    fn queue_dedups_and_prefers_front() {
        let mut q = Queue::default();
        q.push_back(1);
        q.push_back(2);
        q.push_back(1);
        q.push_front(2);
        assert_eq!(q.next(), Some((2, false)));
        assert_eq!(q.next(), Some((1, true)));
        assert_eq!(q.next(), None);
    }

    #[test]
    fn paused_holds_backfill_but_runs_front_items() {
        let mut q = Queue::default();
        q.set_paused(true);
        q.push_back(1);
        q.push_back(2);
        q.push_front(3);
        // 已在队中的回填项被插队后改为非回填
        q.push_front(2);
        assert_eq!(q.next(), Some((2, false)));
        assert_eq!(q.next(), Some((3, false)));
        assert_eq!(q.next(), None);
        q.set_paused(false);
        assert_eq!(q.next(), Some((1, true)));
    }

    #[test]
    fn disable_marks_pending_as_skipped() {
        let mut q = Queue::default();
        q.push_back(1);
        q.push_front(2);
        assert_eq!(q.drain(), [2, 1]);
        assert_eq!(q.next(), None);
    }
}
