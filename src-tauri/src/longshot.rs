//! 长截图（M23 规则 13）：截图选好区域后按 L，遮罩撤下，选区四角显示边框（录屏边框窗口 `rec-frame`）与提示；
//! 用户自己滚动区域里的内容，后台线程不停截这块区域、拼接（`xpaste_core::stitch`）。回车或再按截图快捷键完成，
//! 拼好的图按普通写入写剪切板（同截图复制，进历史），系统通知结果；Esc 取消。
//!
//! 选区旁有拼接结果的预览（最下面最新接上的部分），边框页收到进度后用 `long_preview` 拉取，看接缝对不对。
//!
//! 期间临时把回车、Esc 注册成全局快捷键（键盘已还给原应用，否则收不到），结束时注销。
//! 截屏排除边框窗口：macOS 只截它之下的窗口，Windows 上它带 `WDA_EXCLUDEFROMCAPTURE`（见 `rec::show_frame`）。

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
use xpaste_core::stitch::{Step, Stitch};
use xpaste_platform::WinRect;

use crate::error::AppError;
use crate::events;
use crate::rec;
use crate::shot::{self, LRect};

/// 两次截屏的最小间隔。截一次本身要 50–80 ms（macOS 2 倍屏 800 × 600 到 1440 × 900 点），实际几乎是截完就截：
/// 间隔越短，滚得快时两帧之间越不容易超过一屏（真机验收时 100 ms 间隔常常接不上）
const TICK: Duration = Duration::from_millis(40);
/// 比对时忽略区域右侧这么宽（逻辑像素）：滚动条随滚动移动，整列对不上
const SCROLLBAR: f64 = 24.0;
/// 每帧底部这么高（逻辑像素）先不接，滚上来再接：选中整个窗口时底部有圆角，列表底部常有渐隐
const GUARD: f64 = 40.0;
/// 连续这么久都没接上，提示改为“这里接不上”，告诉用户回车可以保留已拼好的部分
const STUCK: Duration = Duration::from_secs(3);
const ENTER: &str = "Enter";
const ESC: &str = "Escape";

const RUN: u8 = 0;
const DONE: u8 = 1;
const CANCEL: u8 = 2;

struct Run {
    cmd: Arc<AtomicU8>,
    rect: LRect,
    /// 截屏线程拿到第一帧后放进来；预览在命令线程里读，截屏线程拼接时短暂加锁
    st: Arc<Mutex<Option<Stitch>>>,
}

static RUNNING: Mutex<Option<Run>> = Mutex::new(None);

fn running() -> std::sync::MutexGuard<'static, Option<Run>> {
    RUNNING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 发给边框页（`rec-frame`）的进度：已拼接的高度（物理像素）与这一帧的情况
#[derive(Serialize, Clone)]
pub struct Progress {
    height: usize,
    /// `run` 正常；`lost` 这一帧没对上（滚得太快）；`stuck` 连续 `STUCK` 都没对上（画面一直在变，或这个应用的滚动
    /// 落在小数像素上，见已知限制）；`full` 已到长度上限
    state: &'static str,
}

pub fn active() -> bool {
    running().is_some()
}

/// 边框位置（边框页刚建好时拉取）；不在长截图时为 `None`。
pub fn frame_rect() -> Option<LRect> {
    running().as_ref().map(|r| r.rect)
}

/// `shot_long_start`：阻塞线程里执行。只接受当前这一次截图会话、由第 `index` 块屏的遮罩发起的调用。
pub fn start(app: &AppHandle, generation: u64, index: usize, rect: LRect) -> Result<(), AppError> {
    let finite = [rect.x, rect.y, rect.w, rect.h]
        .iter()
        .all(|n| n.is_finite());
    if !finite || rect.w < 1.0 || rect.h < 1.0 {
        return Err(AppError::Invalid("长截图区域无效".into()));
    }
    // 结束会话会清掉屏幕信息：先在同一次加锁里核对代数并取出
    let (origin, scale, bounds) = {
        let s = shot::session(app);
        match (s.screen(index), s.screen_bounds(index)) {
            (Some((o, k)), Some(b)) if s.is_current(generation) => (o, k, b),
            _ => return Err(AppError::Invalid("截图已结束".into())),
        }
    };
    let cmd = Arc::new(AtomicU8::new(RUN));
    let st = Arc::new(Mutex::new(None));
    {
        let mut r = running();
        if r.is_some() || rec::is_busy(app) {
            return Err(AppError::Invalid("正在长截图或录屏".into()));
        }
        *r = Some(Run {
            cmd: cmd.clone(),
            rect,
            st: st.clone(),
        });
    }
    // 撤下遮罩、把键盘还给原应用；随后边框上屏（主线程上排在隐藏遮罩之后）
    shot::end_if_current(app, generation);
    let below = rec::show_frame(app, bounds);
    events::emit_to(app, rec::FRAME, events::REC_FRAME, Some(rect));
    let gs = app.global_shortcut();
    for k in [ENTER, ESC] {
        if let Err(e) = gs.register(k) {
            log::warn!("long 注册 {k} 失败：{e}");
        }
    }
    let (o, size) = shot::pin_bounds(origin, scale, rect, shot::POINTS);
    let native = WinRect {
        x: o.0,
        y: o.1,
        w: size.0,
        h: size.1,
    };
    log::info!("long 开始 屏 {index} {}x{}", rect.w, rect.h);
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || run(&app, &cmd, &st, native, below, scale));
    Ok(())
}

/// `long_preview`：拼接结果缩到宽 `w`、最多高 `h`（物理像素）的预览，前 8 字节为小端 `u32` 宽、高，其后是 RGBA。
/// 不在长截图或还没截到第一帧时为 `None`。
pub fn preview(w: u32, h: u32) -> Option<Vec<u8>> {
    let st = running().as_ref()?.st.clone();
    let g = st.lock().unwrap_or_else(PoisonError::into_inner);
    // 预览最大 400 × 4000：边框页按屏幕尺寸要，这里只防异常参数
    let (pw, ph, px) = g
        .as_ref()?
        .preview(w.min(400) as usize, h.min(4000) as usize);
    let mut bytes = Vec::with_capacity(8 + px.len());
    bytes.extend_from_slice(&(pw as u32).to_le_bytes());
    bytes.extend_from_slice(&(ph as u32).to_le_bytes());
    bytes.extend_from_slice(&px);
    Some(bytes)
}

/// 完成（`done`：回车、截图快捷键）或取消（Esc）。拼图与收尾在截屏线程里做。
pub fn stop(done: bool) {
    if let Some(r) = running().as_ref() {
        r.cmd
            .store(if done { DONE } else { CANCEL }, Ordering::SeqCst);
        log::info!("long {}", if done { "完成" } else { "取消" });
    }
}

/// 全局快捷键回调里先问这里：长截图期间的回车、Esc 归长截图。返回是否已处理。
pub fn key(pressed: &Shortcut) -> bool {
    if !active() {
        return false;
    }
    let is = |k: &str| k.parse::<Shortcut>().is_ok_and(|s| s == *pressed);
    if is(ENTER) {
        stop(true);
    } else if is(ESC) {
        stop(false);
    } else {
        return false;
    }
    true
}

/// 截屏线程：第一帧作为开头，之后每 `TICK` 截一次、拼接，直到完成或取消。
fn run(
    app: &AppHandle,
    cmd: &AtomicU8,
    shared: &Mutex<Option<Stitch>>,
    r: WinRect,
    below: Option<u32>,
    scale: f64,
) {
    let grab = || xpaste_platform::capture_rect(r, below);
    // 遮罩刚撤下：等窗口服务器真正撤掉它，第一帧里不能有蒙层（同截图的 settle）
    std::thread::sleep(Duration::from_millis(50));
    let (w, h, first) = match grab() {
        Ok(f) => f,
        Err(e) => {
            log::warn!("long 截屏失败：{e}");
            end(app);
            return rec::frame_result(app, "长截图失败", false);
        }
    };
    let (w, h) = (w as usize, h as usize);
    let px = |v: f64| (v * scale).round() as usize;
    let stitch = || shared.lock().unwrap_or_else(PoisonError::into_inner);
    *stitch() = Some(Stitch::new(w, h, &first, px(SCROLLBAR), px(GUARD)));
    let height = || stitch().as_ref().map_or(0, Stitch::height);
    let progress = |height, state| {
        events::emit_to(
            app,
            rec::FRAME,
            events::LONG_SHOT,
            Progress { height, state },
        );
    };
    // 进度在高度或状态变了时发，另外每秒补发一次：边框页刚建好时可能错过第一条
    let (mut sent, mut state) = (None, "run");
    // 从哪一刻起一直没接上
    let mut lost_since: Option<Instant> = None;
    let mut warned = false;
    // 排查用：截了几帧、几帧没接上、截屏累计耗时
    let (mut frames, mut lost, mut grab_ms) = (0u32, 0u32, 0u128);
    for tick in 0u32.. {
        let t = Instant::now();
        let c = cmd.load(Ordering::SeqCst);
        if c == CANCEL {
            end(app);
            return rec::hide_frame_after_fade(app, Instant::now());
        }
        // 按回车时也再截一帧：最后滚进来的内容不能漏
        let g = grab();
        frames += 1;
        grab_ms += t.elapsed().as_millis();
        match g {
            // 尺寸变了（如分辨率切换）：这一帧跳过
            Ok((fw, fh, px)) if fw as usize == w && fh as usize == h => {
                let step = stitch().as_mut().map(|st| st.push(px));
                state = match step.unwrap_or(Step::Same) {
                    // 滚回接上的位置：画面与上次接上的一样，“没接上”的提示要撤掉
                    Step::Same | Step::Added(_) => "run",
                    Step::Lost => {
                        lost += 1;
                        let since = *lost_since.get_or_insert(t);
                        if since.elapsed() >= STUCK {
                            "stuck"
                        } else {
                            "lost"
                        }
                    }
                    Step::Full => "full",
                };
                if state != "lost" && state != "stuck" {
                    lost_since = None;
                }
            }
            Ok(_) => {}
            Err(e) if !warned => {
                warned = true;
                log::warn!("long 截屏失败：{e}");
            }
            Err(_) => {}
        }
        if c == DONE {
            break;
        }
        let now = (height(), state);
        if sent != Some(now) || tick % 10 == 0 {
            sent = Some(now);
            progress(now.0, now.1);
        }
        std::thread::sleep(TICK.saturating_sub(t.elapsed()));
    }
    end(app);
    let Some(st) = stitch().take() else {
        return;
    };
    let (w, h, rgba) = st.finish();
    let t = Instant::now();
    let (notice, ok) = match encode(w, h, rgba).and_then(|png| {
        let n = png.len();
        shot::copy_png(png).map(|()| n)
    }) {
        Ok(bytes) => {
            log::info!(
                "long 完成 {w}x{h} {bytes} 字节 编码与写剪切板 {}ms；截屏 {frames} 帧 平均 {}ms，没接上 {lost} 帧",
                t.elapsed().as_millis(),
                grab_ms / u128::from(frames.max(1))
            );
            // 超过 M01 的大小上限，监听器不入库
            let text = if bytes > xpaste_core::capture::MAX_BITMAP_BYTES {
                format!("长截图已复制，超过 50 MB，没有加入历史 · {w} × {h}")
            } else {
                format!("长截图已复制，已加入历史 · {w} × {h}")
            };
            (text, true)
        }
        Err(e) => {
            log::warn!("long 输出失败：{e:?}");
            ("长截图失败".into(), false)
        }
    };
    rec::frame_result(app, &notice, ok);
}

fn encode(w: usize, h: usize, rgba: Vec<u8>) -> Result<Vec<u8>, AppError> {
    let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba)
        .ok_or_else(|| AppError::Invalid("长截图尺寸无效".into()))?;
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| AppError::Invalid(format!("长截图编码失败：{e}")))?;
    Ok(png)
}

/// 撤下边框（淡出，窗口留着显示结果；取消时由调用方隐藏）、注销回车与 Esc、回到空闲。
fn end(app: &AppHandle) {
    rec::frame_out(app);
    let gs = app.global_shortcut();
    for k in [ENTER, ESC] {
        if let Err(e) = gs.unregister(k) {
            log::debug!("long 注销 {k} 失败：{e}");
        }
    }
    *running() = None;
}
