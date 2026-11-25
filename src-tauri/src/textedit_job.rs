//! 改字的一张图：读字、查这一处能不能改、算怎么改、存改动。
//!
//! 一张打开的图是一个 `Live`：像素不变；读字和给不给改在后台线程判断一次，要用的等它；改动和候选在 `St` 里，
//! 写盘拿着它的锁排队。第一处改动提交时才落盘：不给改的、没改过的不写。
//!
//! 页面给的都在这里重新查（`Live::check`）。坐标：四角、条带的意思见 `xpaste_pro::types`，
//! `range` 一律是 `te_strip(四角)` 那条条带里的 x，可以伸出条带（加字写到行尾外）。
//!
//! 锁的先后：`Live::st` → 任务列表 → 名额。

use std::io::ErrorKind;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;
use xpaste_platform::{OcrError, OcrLine};
use xpaste_pro::{TeBlock, TeImg, TeLocal, TePaste, TeQuad, TeSource, TeStrip, TeVerdict};

use crate::error::AppError;
use crate::housekeeping::{self, UNDO_WINDOW};
use crate::textedit_store::{self as store, Align, Edit, Job, NO_PICK};
use crate::textedit_tasks::Tasks;

/// 一行最多几个字；读出来就更长的行，以它的字数为限
const MAX: usize = 60;
const OCR_SIDE: u32 = 8000;

pub const UNREADABLE: &str = "这张图读不出来";
const BLOCKED: &str = "这张图不支持改字";
const BLOCKED_TEXT: &str = "这行字不支持改";
pub const LOCKED: &str = "签名的那一段改不了";
const NOT_CHANGED: &str = "新旧一样，或者是空的";
const TOO_LONG: &str = "一行最多 60 个字";
const BAD_RANGE: &str = "改动范围不对";
const NO_LOCAL: &str = "本机改不了这一处";
pub const GONE: &str = "这张图已经从“最近改过”里删掉了";
const NOT_SAVED: &str = "没存成，请重试";
const NO_EDIT: &str = "这一处已经不在了";
const NOTHING: &str = "还没有改好的";
pub const MISREAD: &str = "字可能不对，看一下";

pub fn no(msg: &str) -> AppError {
    AppError::Invalid(msg.into())
}

/// 存盘出错的原因：文件夹不在了（删掉、清空了）和别的
fn not_saved_text(e: &std::io::Error) -> &'static str {
    if e.kind() == ErrorKind::NotFound {
        return GONE;
    }
    log::warn!("改字 存盘失败：{:?}", e.kind());
    NOT_SAVED
}

pub fn not_saved(e: std::io::Error) -> AppError {
    no(not_saved_text(&e))
}

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 带位置的识别；`Ok(None)` 是没有能用的引擎
pub type Ocr = fn(&[u8]) -> Result<Option<Vec<OcrLine>>, OcrError>;

pub fn ocr(png: &[u8]) -> Result<Option<Vec<OcrLine>>, OcrError> {
    xpaste_platform::ocr_lines(png, OCR_SIDE)
}

/// 读出来的字和给不给改。带用户文字，不派生 Debug
pub struct Read {
    pub lines: Vec<OcrLine>,
    pub blocked: Option<TeBlock>,
    /// 签名的那几段，裁在图内
    pub locked: Vec<TeQuad>,
    /// 有没有识别引擎（识别出错的算有）
    pub engine: bool,
    /// 图解不开、读字的线程起不来，给不给改没判断成：不许改。识别引擎出错的不算
    pub failed: bool,
}

impl Read {
    fn new(img: &TeImg, ocr: Ocr) -> Read {
        let png = crate::img_meta::encode_png(img.w, img.h, &img.px, None);
        let got = png.map_or(Err(OcrError::Decode), |p| ocr(&p));
        let failed = matches!(got, Err(OcrError::Decode));
        let (lines, engine) = match got {
            Ok(Some(l)) => (l, true),
            Ok(None) => (Vec::new(), false),
            Err(_) => (Vec::new(), true),
        };
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        let rows: Vec<(String, TeQuad)> = lines.iter().map(|l| (l.text.clone(), l.quad)).collect();
        let locked = xpaste_pro::te_signature_spans(&rows);
        Read {
            blocked: xpaste_pro::te_block(&texts, img),
            locked: locked.iter().map(|q| clip(q, img.w, img.h)).collect(),
            lines,
            engine,
            failed,
        }
    }
}

/// 一张图的改动。拿着它的锁改、存盘，同一张图的写盘就排着队
struct St {
    job: Job,
    stored: bool,
    rev: u64,
    /// 没存进去的结果，有机会就再存：花了钱，不能丢
    late: Vec<Late>,
}

/// 一个要存的 AI 结果
pub struct Late {
    pub task: u64,
    pub edit: u64,
    pub paste: TePaste,
    pub note: &'static str,
    pub placed: bool,
    pub pick: bool,
}

pub struct Live {
    pub id: String,
    pub img: TeImg,
    pub icc: Option<Vec<u8>>,
    read: OnceLock<Read>,
    st: Mutex<St>,
    /// 最近一次算的本机做法
    local: Mutex<Option<(Spot, Option<Arc<TeLocal>>)>>,
}

/// 给页面的一张图；`rev` 整个进程只增，晚到的旧快照页面丢掉
#[derive(Clone, Serialize)]
pub struct JobView {
    pub id: String,
    pub rev: u64,
    pub w: u32,
    pub h: u32,
    pub edits: Vec<Edit>,
}

fn next_rev() -> u64 {
    static REV: AtomicU64 = AtomicU64::new(0);
    REV.fetch_add(1, Ordering::Relaxed) + 1
}

impl Live {
    fn new(img: TeImg, icc: Option<Vec<u8>>, job: Job, stored: bool) -> Live {
        Live {
            id: job.id.clone(),
            img,
            icc,
            read: OnceLock::new(),
            st: Mutex::new(St {
                job,
                stored,
                rev: next_rev(),
                late: Vec::new(),
            }),
            local: Mutex::new(None),
        }
    }

    /// 刚解出来的一张
    pub fn fresh(img: TeImg, icc: Option<Vec<u8>>) -> Live {
        let job = Job::new(&img);
        Live::new(img, icc, job, false)
    }

    /// 从“最近改过”读回来
    pub fn load(dir: &Path, id: &str) -> std::io::Result<Live> {
        let job = store::load(dir, id)?;
        let img = store::load_orig(dir, &job)?;
        Ok(Live::new(img, store::load_icc(dir, id), job, true))
    }

    /// 读字、判断给不给改，耗时
    pub fn start(&self, ocr: Ocr) {
        let t = Instant::now();
        let r = self.read.get_or_init(|| Read::new(&self.img, ocr));
        log::info!(
            "改字 读字 {} 行 锁住 {} 段{}{} 耗时 {} ms",
            r.lines.len(),
            r.locked.len(),
            if r.engine {
                ""
            } else {
                "（没有识别引擎）"
            },
            r.blocked
                .map_or(String::new(), |b| format!(" 不给改：{}", b.label())),
            t.elapsed().as_millis()
        );
    }

    /// 读字的线程起不来
    pub fn no_reading(&self) {
        let _ = self.read.set(Read {
            lines: Vec::new(),
            blocked: None,
            locked: Vec::new(),
            engine: false,
            failed: true,
        });
    }

    /// 读字的结果，没读完就等
    pub fn read(&self) -> &Read {
        self.read.wait()
    }

    pub fn view(&self) -> JobView {
        let st = lock(&self.st);
        JobView {
            id: self.id.clone(),
            rev: st.rev,
            w: self.img.w,
            h: self.img.h,
            edits: st.job.edits.clone(),
        }
    }

    /// 没落过盘的先落盘，上次没存进去的再存一次
    fn settle(&self, t: Tasks, dir: &Path, st: &mut St) -> std::io::Result<()> {
        if !st.stored {
            store::create(dir, &st.job, &self.img, self.icc.as_deref())?;
            st.stored = true;
        }
        while !st.late.is_empty() {
            let l = st.late.remove(0);
            if let Err(e) = add(dir, st, &l) {
                st.late.insert(0, l);
                return Err(e);
            }
            t.saved(l.task, (!l.placed).then_some(l.note));
        }
        lock(&KEPT).retain(|k| k.live.id != self.id);
        Ok(())
    }

    /// 改并存盘：`f` 在副本上改、自己存，成了才算数
    pub fn commit<T>(
        &self,
        t: Tasks,
        dir: &Path,
        f: impl FnOnce(&mut Job) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let mut st = lock(&self.st);
        self.settle(t, dir, &mut st).map_err(not_saved)?;
        let mut job = st.job.clone();
        let out = f(&mut job)?;
        st.job = job;
        st.rev = next_rev();
        Ok(out)
    }

    /// 任务结束：存结果（选中它），再让任务结束；一直拿着锁，途中取消的不选中。没存成的留着等下次。
    /// `why` 是没改出来的原因，这时也可能有要存的（没贴好的）。返回改好了没有
    pub fn land(
        self: &Arc<Self>,
        t: Tasks,
        dir: &Path,
        task: u64,
        late: Option<Late>,
        why: Option<String>,
    ) -> bool {
        let mut st = lock(&self.st);
        let mut before = NO_PICK;
        let saved = late.as_ref().map_or(Ok(()), |l| {
            // 先补存的可能改了选中的
            self.settle(t, dir, &mut st)?;
            let e = st.job.edits.iter().find(|e| e.id == l.edit);
            before = e.map_or(NO_PICK, |e| e.pick);
            add(dir, &mut st, l)
        });
        let r = match (&saved, why) {
            (Err(e), _) => Err(not_saved_text(e).to_owned()),
            (_, Some(why)) => Err(why),
            _ => Ok(()),
        };
        let done = t.finish(task, r);
        let Some(mut l) = late else {
            return done == Some(true);
        };
        let canceled = done.is_none();
        match saved {
            Err(_) => {
                l.pick &= !canceled;
                st.late.push(l);
                keep(self, t, dir);
            }
            Ok(()) if canceled && l.pick => {
                let mut job = st.job.clone();
                if let Some(e) = job.edits.iter_mut().find(|e| e.id == l.edit) {
                    e.pick = before;
                }
                if let Err(e) = store::save(dir, &job) {
                    log::warn!("改字 存盘失败：{:?}", e.kind());
                }
                st.job = job;
                st.rev = next_rev();
            }
            Ok(()) => {}
        }
        done == Some(true)
    }

    /// 没存进去的再存一次
    fn retry_late(&self, t: Tasks, dir: &Path) {
        let mut st = lock(&self.st);
        if !st.late.is_empty()
            && let Err(e) = self.settle(t, dir, &mut st)
        {
            log::warn!("改字 补存失败：{:?}", e.kind());
        }
    }

    /// 这个任务的结果还没存进去的，现在存。没有为 None
    pub fn save_late(&self, t: Tasks, dir: &Path, task: u64) -> Option<Result<(), AppError>> {
        let mut st = lock(&self.st);
        st.late
            .iter()
            .any(|l| l.task == task)
            .then(|| self.settle(t, dir, &mut st).map_err(not_saved))
    }

    /// 落过盘，文件夹没了：删了
    pub fn gone(&self, dir: &Path) -> bool {
        lock(&self.st).stored && !store::exists(dir, &self.id)
    }

    /// 改动以盘上的为准（读进来之后，登记之前，别处可能改过）
    pub fn reload(&mut self, dir: &Path) {
        if let Ok(job) = store::load(dir, &self.id) {
            let st = self.st.get_mut().unwrap_or_else(PoisonError::into_inner);
            st.job = job;
        }
    }

    /// 清空了“最近改过”：候选都没了，再改、再出图时重新落盘；正好没存进去的结果现在存
    pub fn wiped(&self, t: Tasks, dir: &Path) {
        let mut st = lock(&self.st);
        if st.stored && !store::exists(dir, &self.id) {
            st.stored = false;
            for e in &mut st.job.edits {
                e.cands.clear();
                e.pick = NO_PICK;
            }
            st.rev = next_rev();
        }
        drop(st);
        self.retry_late(t, dir);
    }

    /// 画上选中的候选（不带元数据，只给页面看）
    pub fn render(&self, dir: &Path) -> std::io::Result<Vec<u8>> {
        let (job, stored) = {
            let st = lock(&self.st);
            (st.job.clone(), st.stored)
        };
        if !stored {
            return self.orig();
        }
        let img = store::render(dir, &job)?;
        crate::img_meta::encode_png(img.w, img.h, &img.px, self.icc.as_deref())
    }

    /// 成品：只画选中的，在改的、没贴好的不算。等读字完：不给改的不出
    pub fn output(&self, dir: &Path) -> Result<Output, AppError> {
        if self.read().blocked.is_some() {
            return Err(no(BLOCKED));
        }
        let job = lock(&self.st).job.clone();
        let done = job.edits.iter().filter(|e| e.shown().is_some()).count();
        if done == 0 {
            return Err(no(NOTHING));
        }
        let img = store::render(dir, &job).map_err(|_| no(GONE))?;
        let ai = job.ai();
        Ok(Output { img, ai, done })
    }

    /// 原图的 PNG
    pub fn orig(&self) -> std::io::Result<Vec<u8>> {
        crate::img_meta::encode_png(self.img.w, self.img.h, &self.img.px, self.icc.as_deref())
    }

    pub fn cand_png(&self, dir: &Path, file: &str) -> std::io::Result<Vec<u8>> {
        let job = lock(&self.st).job.clone();
        store::cand_png(dir, &job, file)
    }
}

/// 一张成品
pub struct Output {
    pub img: TeImg,
    /// 画上去的里面有没有 AI 出的
    pub ai: bool,
    /// 改好几处
    pub done: usize,
}

/// 加进候选，按 `pick` 选中
fn add(dir: &Path, st: &mut St, l: &Late) -> std::io::Result<()> {
    let mut job = st.job.clone();
    let e = job.edits.iter_mut().find(|e| e.id == l.edit);
    let e = e.ok_or(ErrorKind::NotFound)?;
    if l.pick {
        e.pick = e.cands.len();
    }
    store::add_cand(
        dir,
        &mut job,
        l.edit,
        &l.paste,
        store::By::Ai,
        l.note,
        l.placed,
    )?;
    st.job = job;
    st.rev = next_rev();
    Ok(())
}

/// 有结果没存进去的图
struct Kept {
    live: Arc<Live>,
    /// 最近一次没存进去
    at: Instant,
}

static KEPT: Mutex<Vec<Kept>> = Mutex::new(Vec::new());

/// 隔多久补存一次，比撤销期长
const KEEP_FOR: Duration = if cfg!(test) {
    Duration::from_secs(1)
} else {
    UNDO_WINDOW.saturating_add(Duration::from_secs(1))
};
/// 自己补存几次，之后等下一次操作
const KEEP_TRIES: u32 = 20;

/// 撤销了删除：没存进去的再存一次
pub fn retry_late(t: Tasks, dir: &Path, id: &str) {
    let live = lock(&KEPT)
        .iter()
        .find(|k| k.live.id == id)
        .map(|k| k.live.clone());
    if let Some(l) = live {
        l.retry_late(t, dir);
    }
}

/// 留在内存里，过一会儿自己补存
fn keep(live: &Arc<Live>, t: Tasks, dir: &Path) {
    let at = Instant::now();
    {
        let mut kept = lock(&KEPT);
        kept.retain(|k| k.live.id != live.id);
        kept.push(Kept {
            live: live.clone(),
            at,
        });
    }
    let (id, dir) = (live.id.clone(), dir.to_path_buf());
    housekeeping::spawn("xpaste-textedit-kept", move || {
        let (mut tries, mut missing) = (0, false);
        while tries < KEEP_TRIES || missing {
            tries += 1;
            std::thread::sleep(KEEP_FOR);
            if !tend(t, &dir, &id, at, &mut missing) {
                return;
            }
        }
    });
}

/// 补存一次，返回还要不要接着管。文件夹没了的再等一回：可能刚删，还能撤销
fn tend(t: Tasks, dir: &Path, id: &str, at: Instant, missing: &mut bool) -> bool {
    let live = {
        let mut kept = lock(&KEPT);
        // 存进去了，或者又没存进去一次（有新的线程管）
        let Some(i) = kept.iter().position(|k| k.live.id == id && k.at == at) else {
            return false;
        };
        if !store::exists(dir, id) {
            if *missing {
                kept.remove(i);
            }
            return !std::mem::replace(missing, true);
        }
        *missing = false;
        kept[i].live.clone()
    };
    live.retry_late(t, dir);
    crate::textedit_tasks::saved_later();
    true
}

// ---- 几何 ----

type Pts = Vec<[f64; 2]>;

fn pts(q: &TeQuad) -> Pts {
    q.iter().map(|p| p.map(f64::from)).collect()
}

/// 有正负的面积
fn area(p: &[[f64; 2]]) -> f64 {
    let n = p.len();
    (0..n)
        .map(|i| (p[i], p[(i + 1) % n]))
        .map(|(a, b)| a[0] * b[1] - a[1] * b[0])
        .sum::<f64>()
        / 2.0
}

/// 重叠的面积：`a` 依次用 `b` 的四条边切。四角要成凸四边形
fn overlap(a: &TeQuad, b: &TeQuad) -> f64 {
    let b = pts(b);
    let dir = area(&b).signum();
    let mut p = pts(a);
    for i in 0..4 {
        let ([x, y], [u, v]) = (b[i], b[(i + 1) % 4]);
        let side = |c: [f64; 2]| dir * ((u - x) * (c[1] - y) - (v - y) * (c[0] - x));
        let mut out = Pts::new();
        for k in 0..p.len() {
            let (c, d) = (p[k], p[(k + 1) % p.len()]);
            let (m, n) = (side(c), side(d));
            if m >= 0.0 {
                out.push(c);
            }
            if (m < 0.0) != (n < 0.0) {
                let f = m / (m - n);
                out.push([c[0] + (d[0] - c[0]) * f, c[1] + (d[1] - c[1]) * f]);
            }
        }
        p = out;
    }
    area(&p).abs()
}

/// 点在不在四角里
fn inside(q: &TeQuad, x: f32, y: f32) -> bool {
    let side = |k: usize| {
        let (a, b) = (q[k], q[(k + 1) % 4]);
        (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0])
    };
    (0..4).all(|k| side(k) >= 0.0) || (0..4).all(|k| side(k) <= 0.0)
}

/// 两处改动算不算改在同一个地方：重叠超过较小那个的一成。行距紧时上下两行的框会叠上一点：有一方只看中间七成
fn clash(a: &TeQuad, b: &TeQuad) -> bool {
    let mid = |q: &TeQuad| band(q, 0.15, 0.85);
    let least = 0.7 * area(&pts(a)).abs().min(area(&pts(b)).abs());
    least > 0.0 && overlap(&mid(a), b).max(overlap(a, &mid(b))) > 0.1 * least
}

/// 沿行的左右边之间取一段（0–1 是四角的上下）
fn band(q: &TeQuad, top: f32, bottom: f32) -> TeQuad {
    let at = |a: [f32; 2], b: [f32; 2], f: f32| [0, 1].map(|k| a[k] + (b[k] - a[k]) * f);
    [
        at(q[0], q[3], top),
        at(q[1], q[2], top),
        at(q[1], q[2], bottom),
        at(q[0], q[3], bottom),
    ]
}

/// 右边两个角沿上下边收到图里
fn clip(q: &TeQuad, w: u32, h: u32) -> TeQuad {
    let n = [w as f32, h as f32];
    let pull = |a: [f32; 2], b: [f32; 2]| {
        let mut t = 1f32;
        for k in 0..2 {
            let d = b[k] - a[k];
            if b[k] > n[k] {
                t = t.min((n[k] - a[k]) / d);
            } else if b[k] < 0.0 {
                t = t.min(-a[k] / d);
            }
        }
        let t = t.max(0.0);
        [0, 1].map(|k| (a[k] + (b[k] - a[k]) * t).clamp(0.0, n[k]))
    };
    [q[0], pull(q[0], q[1]), pull(q[3], q[2]), q[3]]
}

/// 条带里的一段（整个高）在原图上的四角
fn span(s: &TeStrip, range: [f32; 2]) -> TeQuad {
    let [a, b, c, d, e, f, g, h, i] = s.m;
    // 伴随矩阵就够了：齐次的，差一个倍数
    let inv = [
        e * i - f * h,
        c * h - b * i,
        b * f - c * e,
        f * g - d * i,
        a * i - c * g,
        c * d - a * f,
        d * h - e * g,
        b * g - a * h,
        a * e - b * d,
    ];
    let y = f64::from(s.img.h);
    let [x0, x1] = range.map(f64::from);
    [(x0, 0.0), (x1, 0.0), (x1, y), (x0, y)].map(|(x, y)| {
        let z = inv[6] * x + inv[7] * y + inv[8];
        [0, 3].map(|k| ((inv[k] * x + inv[k + 1] * y + inv[k + 2]) / z) as f32)
    })
}

/// 掉个头：原来的左边变成右边
fn turn(q: &TeQuad) -> TeQuad {
    [q[2], q[3], q[0], q[1]]
}

fn width(q: &TeQuad) -> f32 {
    let len = |a: [f32; 2], b: [f32; 2]| (b[0] - a[0]).hypot(b[1] - a[1]);
    ((len(q[0], q[1]) + len(q[3], q[2])) / 2.0).max(1.0)
}

/// 左右各外推行宽的几倍，截在图内
fn stretch(img: &TeImg, line: &TeQuad, left: f32, right: f32) -> TeQuad {
    let mut q = *line;
    if right > 0.0 {
        q = xpaste_pro::te_extend(img, q, 1.0 + right);
    }
    if left > 0.0 {
        let k = 1.0 + left * width(line) / width(&q);
        q = turn(&xpaste_pro::te_extend(img, turn(&q), k));
    }
    q
}

/// 把要改的范围都装进去的行，和换到它上面的范围
pub fn wide(img: &TeImg, line: &TeQuad, range: [f32; 2]) -> (TeQuad, [f32; 2]) {
    let s = xpaste_pro::te_strip(img, *line);
    let w = s.img.w.max(1) as f32;
    let (left, right) = (-range[0] / w, range[1] / w - 1.0);
    if left <= 0.0 && right <= 0.0 {
        return (*line, range);
    }
    let q = stretch(img, line, left, right);
    (
        q,
        xpaste_pro::te_remap(&s, &xpaste_pro::te_strip(img, q), range),
    )
}

/// `x` 往右的空白到哪儿，`line` 那条条带（`s`）的 x。行尾外看到行宽再来一倍，短行至少 8 个行高；量不出时是条带右端
fn room(img: &TeImg, line: &TeQuad, s: &TeStrip, x: f32) -> f32 {
    let (w, h) = (s.img.w as f32, s.img.h as f32);
    let far = xpaste_pro::te_extend(img, *line, 1.0 + (8.0 * h / w.max(1.0)).max(1.0));
    let r = if far == *line {
        xpaste_pro::te_room(s, x)
    } else {
        let f = xpaste_pro::te_strip(img, far);
        let r = xpaste_pro::te_room(&f, xpaste_pro::te_remap(s, &f, [x; 2])[0]);
        xpaste_pro::te_remap(&f, s, [r; 2])[0]
    };
    if r.is_finite() && r >= x { r } else { x.max(w) }
}

/// 图里 `[x0, y0, x1, y1]` 的一块
fn cut(img: &TeImg, [x0, y0, x1, y1]: [u32; 4]) -> TeImg {
    let row = |y: u32| {
        let at = (y as usize * img.w as usize + x0 as usize) * 4;
        &img.px[at..at + (x1 - x0) as usize * 4]
    };
    TeImg {
        w: x1 - x0,
        h: y1 - y0,
        px: (y0..y1).flat_map(row).copied().collect(),
    }
}

/// 把 `p` 盖到 `img`（原图从 `(x, y)` 开始的一块）上
fn lay(img: &mut TeImg, (x, y): (u32, u32), p: &TePaste) {
    for j in 0..p.patch.h {
        for i in 0..p.patch.w {
            let (u, v) = ((p.x + i).wrapping_sub(x), (p.y + j).wrapping_sub(y));
            if u < img.w && v < img.h {
                let to = (v as usize * img.w as usize + u as usize) * 4;
                let from = (j as usize * p.patch.w as usize + i as usize) * 4;
                img.px[to..to + 4].copy_from_slice(&p.patch.px[from..from + 4]);
            }
        }
    }
}

// ---- 查 ----

pub fn graphemes(s: &str) -> Vec<&str> {
    s.graphemes(true).collect()
}

/// 改了几个字：去掉相同的头和尾，新旧里多的那个
pub fn changed_count(old: &str, new: &str) -> usize {
    let (a, b) = (graphemes(old), graphemes(new));
    let least = a.len().min(b.len());
    let head = (0..least).take_while(|&i| a[i] == b[i]).count();
    let tail = (0..least - head)
        .take_while(|&i| a[a.len() - 1 - i] == b[b.len() - 1 - i])
        .count();
    a.len().max(b.len()) - head - tail
}

/// 按字宽算有几个字：数字、半角标点和字母算半个
fn weight(s: &[&str]) -> f32 {
    s.iter()
        .map(|g| {
            if g.chars().next().is_some_and(|c| u32::from(c) < 0x250) {
                0.5
            } else {
                1.0
            }
        })
        .sum()
}

/// 页面给的一处改动；`edit` 是从哪一处打开重改的。带用户文字，不派生 Debug
#[derive(Clone, Deserialize)]
pub struct Draft {
    pub edit: Option<u64>,
    pub line: TeQuad,
    pub range: Option<[f32; 2]>,
    pub old: String,
    pub new: String,
    pub align: Align,
}

/// 查过的一处：字去了末尾空白，范围扩到要用的空白
#[derive(Clone, PartialEq)]
pub struct Spot {
    pub edit: Option<u64>,
    pub line: TeQuad,
    pub range: [f32; 2],
    pub old: String,
    pub new: String,
    pub align: Align,
}

impl Spot {
    fn same_count(&self) -> bool {
        graphemes(&self.old).len() == graphemes(&self.new).len()
    }

    /// 提示词里怎么重排
    pub fn realign(&self) -> Option<&'static str> {
        match self.align {
            _ if self.same_count() => None,
            Align::Left => None,
            Align::Center => Some("居中"),
            Align::Right => Some("靠右"),
        }
    }
}

fn finite(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite())
}

/// 扩到新字要占的空白：字数变多的、加字的（空范围）。按原文的平均字宽估，多留半个字
fn grow(img: &TeImg, d: &Draft, s: &TeStrip, range: [f32; 2]) -> [f32; 2] {
    let (o, n) = (graphemes(d.old.trim_end()), graphemes(d.new.trim_end()));
    let [mut a, mut b] = range;
    if n.len() > o.len() || a == b {
        let (w, h) = (s.img.w as f32, s.img.h as f32);
        let one = w / weight(&o).max(0.5);
        let more = ((weight(&n) - weight(&o)).max(0.0) + 0.5) * one;
        let (left, right) = match d.align {
            Align::Left => (0.0, more),
            Align::Center => (more / 2.0, more / 2.0),
            Align::Right => (more, 0.0),
        };
        if right > 0.0 {
            b = (b + right).min(room(img, &d.line, s, b));
        }
        if left > 0.0 {
            let t = turn(&d.line);
            let r = room(img, &t, &xpaste_pro::te_strip(img, t), w - a);
            a = (a - left).max(w - r);
        }
        // 一点空白都没有：按一个字算
        if b - a < 1.0 {
            b = a + h;
        }
    }
    [a, b]
}

impl Live {
    /// 这张图、这几个字给不给改
    fn allow(&self, d: &Draft) -> Result<(), AppError> {
        let r = self.read();
        if r.failed {
            return Err(no(UNREADABLE));
        }
        if r.blocked.is_some() {
            return Err(no(BLOCKED));
        }
        let (old, new) = (d.old.trim_end(), d.new.trim_end());
        if old.trim().is_empty() || new.trim().is_empty() || old == new {
            return Err(no(NOT_CHANGED));
        }
        let n = |s: &str| graphemes(s).len();
        let longest = r.lines.iter().map(|l| n(&l.text)).max().unwrap_or(0);
        if n(old) > MAX.max(longest) || n(new) > MAX.max(n(old)) {
            return Err(no(TOO_LONG));
        }
        if let Some(b) = xpaste_pro::te_block_text(old, new) {
            log::info!("改字 这行字不给改：{}", b.label());
            return Err(no(BLOCKED_TEXT));
        }
        Ok(())
    }

    /// 碰没碰到签名的那一段。挨着的行，框会叠上一点：上下有一方只看中间一半，哪一方的碰到都算（框可以很高）
    fn touches_locked(&self, q: &TeQuad) -> bool {
        let mid = |q: &TeQuad| band(q, 0.25, 0.75);
        self.read()
            .locked
            .iter()
            .any(|l| overlap(&mid(q), l) > 0.5 || overlap(q, &mid(l)) > 0.5)
    }

    /// 真正变了的像素碰没碰到签名的那一段（改动范围外面一圈也会动）。上下边各让一成
    pub fn spoils_locked(&self, p: &TePaste) -> bool {
        let at = |x: u32, y: u32| (y as usize * self.img.w as usize + x as usize) * 4;
        self.read().locked.iter().any(|l| {
            let l = band(l, 0.1, 0.9);
            (0..p.patch.h).any(|j| {
                (0..p.patch.w).any(|i| {
                    let (x, y) = (p.x + i, p.y + j);
                    let k = (j as usize * p.patch.w as usize + i as usize) * 4;
                    x < self.img.w
                        && y < self.img.h
                        && inside(&l, x as f32 + 0.5, y as f32 + 0.5)
                        && self.img.px[at(x, y)..][..4] != p.patch.px[k..][..4]
                })
            })
        })
    }

    /// `range` 在图里，没碰到签名
    fn within(&self, s: &TeStrip, range: [f32; 2]) -> Result<(), AppError> {
        let q = span(s, range);
        let (w, h) = (self.img.w as f32, self.img.h as f32);
        let all = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];
        // NaN 也不算在图里
        if !finite(q.as_flattened()) || overlap(&q, &all) < 1.0 {
            return Err(no(BAD_RANGE));
        }
        if self.touches_locked(&q) {
            return Err(no(LOCKED));
        }
        Ok(())
    }

    /// 后端自己查一遍页面给的一处。等读字完
    pub fn check(&self, d: &Draft) -> Result<Spot, AppError> {
        self.allow(d)?;
        let range = d.range.ok_or_else(|| no(BAD_RANGE))?;
        if !finite(d.line.as_flattened()) || !finite(&range) || range[0] > range[1] {
            return Err(no(BAD_RANGE));
        }
        let s = xpaste_pro::te_strip(&self.img, d.line);
        // 给那一处再出一张的，范围已经扩过
        let same = lock(&self.st)
            .job
            .edits
            .iter()
            .find(|e| d.edit == Some(e.id) && same(e, &spot(d, range)))
            .map(|e| e.range);
        let range = same.unwrap_or_else(|| grow(&self.img, d, &s, range));
        self.within(&s, range)?;
        Ok(spot(d, range))
    }
}

fn spot(d: &Draft, range: [f32; 2]) -> Spot {
    Spot {
        edit: d.edit,
        line: d.line,
        range,
        old: d.old.trim_end().into(),
        new: d.new.trim_end().into(),
        align: d.align,
    }
}

// ---- 改动之间谁替换谁 ----

/// 字、范围和对齐都没变；字数没变的用不着对齐
fn same(e: &Edit, s: &Spot) -> bool {
    let near = |k: usize| (e.range[k] - s.range[k]).abs() < 0.5;
    (&e.old, &e.new, e.line) == (&s.old, &s.new, s.line)
        && (e.align == s.align || s.same_count())
        && near(0)
        && near(1)
}

/// 找到这一处，没有就新开：从哪一处打开、字、范围和对齐都没变的，是给那一处再出一张。返回它的 id 和被它替换的那一处
pub fn put(job: &mut Job, s: &Spot) -> Result<(u64, Option<u64>), AppError> {
    if let Some(id) = s.edit {
        let e = job.edits.iter().find(|e| e.id == id);
        let e = e.ok_or_else(|| no(NO_EDIT))?;
        if same(e, s) {
            return Ok((id, None));
        }
    }
    let id = job.edits.iter().map(|e| e.id).max().unwrap_or(0) + 1;
    job.edits.push(Edit {
        id,
        line: s.line,
        range: s.range,
        old: s.old.clone(),
        new: s.new.clone(),
        align: s.align,
        pick: NO_PICK,
        cands: Vec::new(),
    });
    Ok((id, s.edit))
}

/// 和 `keep` 改在同一个地方的（还有 `also`）都取消选中，返回它们的 id
pub fn replace(img: &TeImg, job: &mut Job, keep: u64, also: Option<u64>) -> Vec<u64> {
    let at = |e: &Edit| span(&xpaste_pro::te_strip(img, e.line), e.range);
    let Some(mine) = job.edits.iter().find(|e| e.id == keep).map(at) else {
        return Vec::new();
    };
    let mut gone = Vec::new();
    for e in job.edits.iter_mut().filter(|e| e.id != keep) {
        if also == Some(e.id) || clash(&at(e), &mine) {
            e.pick = NO_PICK;
            gone.push(e.id);
        }
    }
    gone
}

/// 选中第 `i` 个候选
pub fn pick(t: Tasks, dir: &Path, live: &Live, edit: u64, i: usize) -> Result<(), AppError> {
    let r = live.read();
    if r.failed || r.blocked.is_some() {
        return Err(no(if r.failed { UNREADABLE } else { BLOCKED }));
    }
    live.commit(t, dir, |job| {
        let e = job.edits.iter_mut().find(|e| e.id == edit);
        let e = e.ok_or_else(|| no(NO_EDIT))?;
        if !e.cands.get(i).is_some_and(|c| c.placed) {
            return Err(no(NO_EDIT));
        }
        e.pick = i;
        let gone = replace(&live.img, job, edit, None);
        store::save(dir, job).map_err(not_saved)?;
        t.cancel_edits(&live.id, &gone);
        Ok(())
    })
}

/// 改回去：候选留着，在改的取消
pub fn undo(t: Tasks, dir: &Path, live: &Live, edit: u64) -> Result<(), AppError> {
    live.commit(t, dir, |job| {
        let e = job.edits.iter_mut().find(|e| e.id == edit);
        e.ok_or_else(|| no(NO_EDIT))?.pick = NO_PICK;
        store::save(dir, job).map_err(not_saved)?;
        t.cancel_edits(&live.id, &[edit]);
        Ok(())
    })
}

/// 本机改好的一处：存成候选、选中。返回它的 id
fn place(t: Tasks, dir: &Path, live: &Live, s: &Spot, paste: &TePaste) -> Result<u64, AppError> {
    live.commit(t, dir, |job| {
        let (id, also) = put(job, s)?;
        let gone = replace(&live.img, job, id, also);
        if let Some(e) = job.edits.iter_mut().find(|e| e.id == id) {
            e.pick = e.cands.len();
        }
        store::add_cand(dir, job, id, paste, store::By::Local, "", true).map_err(not_saved)?;
        t.cancel_edits(&live.id, &gone);
        Ok(id)
    })
}

// ---- 本机的做法 ----

/// 走法怎么算的；`soft` 是本机做得了、但可能有痕迹
#[derive(Serialize, Debug, PartialEq)]
pub struct Plan {
    pub way: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soft: Option<bool>,
    pub why: &'static str,
    pub could: Vec<&'static str>,
}

/// `ai`：设置是直接用 AI；`api`：生图接口配好了；`local`：本机做的自检结果，做不了为 None
fn plan_of(ai: bool, api: bool, local: Option<(TeVerdict, &'static str)>) -> Plan {
    let ok = local.filter(|l| l.0 != TeVerdict::Bad).map(|l| l.0);
    let mut could = Vec::new();
    if ok.is_some() {
        could.push("local");
    }
    if api {
        could.push("ai");
    }
    Plan {
        way: match ok {
            Some(TeVerdict::Pass) if !ai => "local",
            Some(TeVerdict::Soft) if !ai => "soft",
            _ => "ai",
        },
        soft: ok.map(|v| v == TeVerdict::Soft),
        why: local.map_or("", |l| l.1),
        could,
    }
}

impl Live {
    pub fn plan(&self, d: &Draft, ai: bool, api: bool) -> Result<Plan, AppError> {
        let l = self.local(&self.check(d)?);
        Ok(plan_of(ai, api, l.map(|l| (l.verdict, l.why))))
    }

    /// 删字、搬字，哪样都不合适为 None
    fn local(&self, s: &Spot) -> Option<Arc<TeLocal>> {
        let mut last = lock(&self.local);
        if let Some((k, l)) = &*last
            && k == s
        {
            return l.clone();
        }
        let (o, n) = (graphemes(&s.old), graphemes(&s.new));
        let l = if n.len() < o.len() {
            // 删末尾几个字；居中、靠右要整行挪
            (s.align == Align::Left && o.starts_with(&n))
                .then(|| xpaste_pro::te_erase(&self.img, s.line, s.range))
                .flatten()
        } else if n.len() == o.len() {
            self.borrow(s, &o, &n)
        } else {
            None
        };
        // 范围外面一圈碰到签名的不能用
        let l = l.filter(|l| !self.spoils_locked(&l.paste)).map(Arc::new);
        *last = Some((s.clone(), l.clone()));
        l
    }

    /// 能搬字的各行，最后是要改的这一行（按用户写的原文，`n` 个字）。签名里的、自己框的和读出来的是同一行的不要
    fn rows(&self, s: &Spot, n: usize) -> Vec<Row> {
        let r = self.read();
        let least = |q: &TeQuad| area(&pts(q)).abs().min(area(&pts(&s.line)).abs());
        let mut rows = Vec::new();
        let mut mine = Vec::new();
        for l in &r.lines {
            if l.quad == s.line && l.chars.len() == n {
                mine.clone_from(&l.chars);
            }
            let same = overlap(&l.quad, &s.line) > 0.5 * least(&l.quad);
            if !same && !r.locked.iter().any(|q| overlap(&l.quad, q) > 0.5) {
                rows.push((l.text.clone(), l.quad, l.chars.clone()));
            }
        }
        rows.push((s.old.clone(), s.line, mine));
        rows
    }

    /// 从同一张图里搬字，最多两个
    fn borrow(&self, s: &Spot, o: &[&str], n: &[&str]) -> Option<TeLocal> {
        let at: Vec<usize> = (0..o.len()).filter(|&i| o[i] != n[i]).collect();
        let mut chars = Vec::new();
        for &i in &at {
            let mut c = n[i].chars();
            chars.push(c.next()?);
            // 几个码点拼成的字不搬
            if c.next().is_some() {
                return None;
            }
        }
        if chars.is_empty() || chars.len() > 2 {
            return None;
        }
        let lines = self.rows(s, o.len());
        let me = lines.len() - 1;
        let strip = xpaste_pro::te_strip(&self.img, s.line);
        let to = targets(&strip, o.len(), &lines[me].2, &at, s.range)?;
        let from = xpaste_pro::te_find_sources(&self.img, &lines, me, &to, &chars)?;
        // 同一行里连着签名的
        let signed = |f: &TeSource| {
            let strip = xpaste_pro::te_strip(&self.img, lines[f.line].1);
            self.touches_locked(&span(&strip, f.range))
        };
        if from.iter().any(signed) {
            return None;
        }
        let step = |img: &TeImg, k: usize| xpaste_pro::te_move(img, &lines, me, to[k], &from[k]);
        if to.len() == 1 {
            return step(&self.img, 0);
        }
        // 先改一个，在改过的图上再改另一个；第二个的来源不能是先改掉的那个
        let uses = |k: usize, other: usize| {
            let (f, t) = (&from[k], to[other]);
            f.line == me && f.range[0] < t[1] && t[0] < f.range[1]
        };
        let order = if !uses(1, 0) {
            [0, 1]
        } else if !uses(0, 1) {
            [1, 0]
        } else {
            return None;
        };
        let a = step(&self.img, order[0])?;
        if a.verdict == TeVerdict::Bad {
            return Some(a);
        }
        let mut img = TeImg {
            w: self.img.w,
            h: self.img.h,
            px: self.img.px.clone(),
        };
        lay(&mut img, (0, 0), &a.paste);
        let b = step(&img, order[1])?;
        lay(&mut img, (0, 0), &b.paste);
        let edge = |k: fn(&TePaste) -> [u32; 4]| [k(&a.paste), k(&b.paste)];
        let [p, q] = edge(|p| [p.x, p.y, p.x + p.patch.w, p.y + p.patch.h]);
        let all = [
            p[0].min(q[0]),
            p[1].min(q[1]),
            p[2].max(q[2]),
            p[3].max(q[3]),
        ];
        let (verdict, why) = (a.verdict, a.why).max((b.verdict, b.why));
        Some(TeLocal {
            paste: TePaste {
                patch: cut(&img, all),
                x: all[0],
                y: all[1],
                exact: false,
                soft: a.paste.soft || b.paste.soft,
            },
            verdict,
            why,
        })
    }
}

/// 一行的文字、四角、逐字框
type Row = (String, TeQuad, Vec<[f32; 2]>);

/// 第 `at` 几个字各自的范围，两头用页面给的（用户可能拖过），都不超出它；有一个够不着就是 None
fn targets(
    s: &TeStrip,
    n: usize,
    chars: &[[f32; 2]],
    at: &[usize],
    range: [f32; 2],
) -> Option<Vec<[f32; 2]>> {
    let one = |&i: &usize| xpaste_pro::te_range(s, n, chars, i, i + 1);
    let mut to: Vec<[f32; 2]> = at.iter().map(one).collect();
    to.first_mut()?[0] = range[0];
    to.last_mut()?[1] = range[1];
    for r in &mut to {
        *r = [r[0].max(range[0]), r[1].min(range[1])];
    }
    to.iter().all(|r| r[0] < r[1]).then_some(to)
}

/// 用本机的做法改这一处
pub fn run_local(t: Tasks, dir: &Path, live: &Live, d: &Draft) -> Result<u64, AppError> {
    let s = live.check(d)?;
    let l = live.local(&s).filter(|l| l.verdict != TeVerdict::Bad);
    place(t, dir, live, &s, &l.ok_or_else(|| no(NO_LOCAL))?.paste)
}

/// 页面重排好的一条：`quad` 是 `strip` 给的四角，`px` 是改过的 RGBA。范围按真正变了的列算，
/// 可能比页面给的宽（字数不变、宽度变了会动到行尾）
pub fn local_patch(
    t: Tasks,
    dir: &Path,
    live: &Live,
    d: &Draft,
    quad: &TeQuad,
    px: &[u8],
) -> Result<u64, AppError> {
    live.allow(d)?;
    if !finite(quad.as_flattened()) || !finite(d.line.as_flattened()) {
        return Err(no(BAD_RANGE));
    }
    let strip = xpaste_pro::te_strip_native(&live.img, *quad);
    let w = strip.img.w as usize;
    if px.len() != strip.img.px.len() || w == 0 {
        return Err(no(BAD_RANGE));
    }
    let (mut lo, mut hi) = (w, 0);
    let old = strip.img.px.as_chunks::<4>().0;
    for (i, (a, b)) in px.as_chunks::<4>().0.iter().zip(old).enumerate() {
        if a[..3] != b[..3] {
            (lo, hi) = (lo.min(i % w), hi.max(i % w + 1));
        }
    }
    if lo >= hi {
        return Err(no(NOT_CHANGED));
    }
    let real = [lo as f32, hi as f32];
    // 交回来的得是这一行的
    let whole = span(&strip, [0.0, w as f32]);
    let least = area(&pts(&whole)).abs().min(area(&pts(&d.line)).abs());
    if overlap(&whole, &d.line) < 0.5 * least {
        return Err(no(BAD_RANGE));
    }
    let line = xpaste_pro::te_strip(&live.img, d.line);
    let mut range = xpaste_pro::te_remap(&strip, &line, real);
    if let Some(r) = d.range.filter(|r| finite(r)) {
        range = [range[0].min(r[0]), range[1].max(r[1])];
    }
    live.within(&line, range)?;
    let paste = xpaste_pro::te_strip_paste(&live.img, &strip, px);
    let paste = paste.ok_or_else(|| no(NOT_CHANGED))?;
    if live.spoils_locked(&paste) {
        return Err(no(LOCKED));
    }
    place(t, dir, live, &spot(d, range), &paste)
}

/// 改好的那一行再读一次，和要写的不一样为真。没有引擎、没读出字的不算
pub fn misread(img: &TeImg, line: &TeQuad, paste: &TePaste, new: &str, ocr: Ocr) -> bool {
    // 只拷这一行附近的一块
    let edge = |k: usize, n: u32| {
        let v = line.iter().map(|p| p[k]);
        let lo = v.clone().fold(f32::MAX, f32::min).floor() - 8.0;
        let hi = v.fold(f32::MIN, f32::max).ceil() + 8.0;
        [lo, hi].map(|v| v.clamp(0.0, n as f32) as u32)
    };
    let ([x0, x1], [y0, y1]) = (edge(0, img.w), edge(1, img.h));
    if x0 >= x1 || y0 >= y1 {
        return false;
    }
    let mut part = cut(img, [x0, y0, x1, y1]);
    lay(&mut part, (x0, y0), paste);
    let s = xpaste_pro::te_strip(&part, line.map(|p| [p[0] - x0 as f32, p[1] - y0 as f32]));
    // 四周垫一圈，字贴着边时不好认
    let (w, h, m) = (s.img.w, s.img.h, s.img.h / 4);
    let (pw, ph) = (w + 2 * m, h + 2 * m);
    let mut px = Vec::with_capacity((pw * ph * 4) as usize);
    for y in 0..ph {
        for x in 0..pw {
            let at = |v: u32, n: u32| v.saturating_sub(m).min(n.saturating_sub(1)) as usize;
            let i = (at(y, h) * w as usize + at(x, w)) * 4;
            px.extend_from_slice(s.img.px.get(i..i + 4).unwrap_or(&[0; 4]));
        }
    }
    let Ok(png) = crate::img_meta::encode_png(pw, ph, &px, None) else {
        return false;
    };
    let Ok(Some(mut lines)) = ocr(&png) else {
        return false;
    };
    lines.sort_by(|a, b| a.quad[0][0].total_cmp(&b.quad[0][0]));
    // 空白、全角半角不算不一样
    let plain = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| match u32::from(c) {
                v @ 0xFF01..=0xFF5E => char::from_u32(v - 0xFEE0).unwrap_or(c),
                _ => c,
            })
            .collect()
    };
    let got: String = lines.iter().map(|l| plain(&l.text)).collect();
    !got.is_empty() && got != plain(new)
}

// ---- 给页面的范围和条带 ----

/// 哪一行：读出来的第几行，或者四角
#[derive(Deserialize)]
#[serde(untagged)]
pub enum LineRef {
    At(usize),
    Quad(TeQuad),
}

impl LineRef {
    /// 四角和逐字框（四角给的没有）
    fn get(&self, live: &Live) -> Result<(TeQuad, Vec<[f32; 2]>), AppError> {
        let got = match self {
            LineRef::At(i) => live.read().lines.get(*i).map(|l| (l.quad, l.chars.clone())),
            LineRef::Quad(q) => finite(q.as_flattened()).then(|| (*q, Vec::new())),
        };
        got.ok_or_else(|| no(BAD_RANGE))
    }
}

/// `range`、`room` 是 `w` × `h` 那条条带里的 x，`m` 是原图到条带的矩阵。`room` 是 `range` 右边的空白到哪儿，可以超出条带
#[derive(Serialize)]
pub struct Ranges {
    range: [f32; 2],
    room: f32,
    w: u32,
    h: u32,
    m: [f64; 9],
}

pub fn ranges(
    live: &Live,
    line: &LineRef,
    n: usize,
    chars: Vec<[f32; 2]>,
    from: usize,
    to: usize,
) -> Result<Ranges, AppError> {
    let (q, own) = line.get(live)?;
    let chars = if chars.is_empty() { own } else { chars };
    let s = xpaste_pro::te_strip(&live.img, q);
    let range = xpaste_pro::te_range(&s, n, &chars, from, to);
    Ok(Ranges {
        range,
        room: room(&live.img, &q, &s, range[1]),
        w: s.img.w,
        h: s.img.h,
        m: s.m,
    })
}

/// 左右要多取多少（原尺寸条带里的像素）
#[derive(Deserialize, Default, Clone, Copy)]
pub struct Extend {
    left: f32,
    right: f32,
}

/// 条带的说明；`quad` 是它的四角（交回改过的条带时带上），`left`、`right` 是实际多取了多少
#[derive(Serialize)]
pub struct StripHead {
    w: u32,
    h: u32,
    m: [f64; 9],
    quad: TeQuad,
    left: f32,
    right: f32,
}

/// 原尺寸的条带
pub fn strip(
    live: &Live,
    line: &LineRef,
    extend: Extend,
) -> Result<(StripHead, Vec<u8>), AppError> {
    let (q, _) = line.get(live)?;
    let img = &live.img;
    let s = xpaste_pro::te_strip_native(img, q);
    let w = s.img.w as f32;
    let part = |v: f32| if v > 0.0 { v / w.max(1.0) } else { 0.0 };
    let quad = stretch(img, &q, part(extend.left), part(extend.right));
    if quad == q {
        return Ok((head(&s, q, 0.0, 0.0), s.img.px));
    }
    let e = xpaste_pro::te_strip_native(img, quad);
    let [a, b] = xpaste_pro::te_remap(&s, &e, [0.0, w]);
    Ok((head(&e, quad, a, e.img.w as f32 - b), e.img.px))
}

fn head(s: &TeStrip, quad: TeQuad, left: f32, right: f32) -> StripHead {
    StripHead {
        w: s.img.w,
        h: s.img.h,
        m: s.m,
        quad,
        left: left.max(0.0),
        right: right.max(0.0),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::textedit_tasks::tests::tasks as new_tasks;
    use std::path::PathBuf;

    /// 测试用的文件夹，用完删掉
    pub struct Tmp(pub PathBuf);
    impl Tmp {
        pub fn new() -> Self {
            let d = std::env::temp_dir().join(format!("xpaste-ted-{}", crate::meme_gen::uuid()));
            std::fs::create_dir_all(&d).unwrap();
            Self(d)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub const OLD: &str = "会议改到三点";
    pub const NEW: &str = "会议改到四点";
    pub const LINE: TeQuad = [[76.0, 64.0], [428.0, 64.0], [428.0, 126.0], [76.0, 126.0]];

    /// 一行 6 个字（每个是一竖一横），浅底深字
    pub fn scene() -> TeImg {
        let (w, h) = (640i32, 200);
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let k = (x - 80).div_euclid(60);
                let (u, v) = ((x - 80).rem_euclid(60), y - 70);
                let on = (0..6).contains(&k)
                    && (0..50).contains(&v)
                    && ((k * 7..k * 7 + 8).contains(&u)
                        || (u < 44 && (k * 8..k * 8 + 8).contains(&v)));
                px.extend(if on {
                    [20, 24, 30, 255]
                } else {
                    [240, 238, 232, 255]
                });
            }
        }
        TeImg {
            w: w as u32,
            h: h as u32,
            px,
        }
    }

    fn chars() -> Vec<[f32; 2]> {
        (0..6)
            .map(|i| [i as f32 * 60.0 / 352.0, (i as f32 * 60.0 + 52.0) / 352.0])
            .collect()
    }

    pub fn row(text: &str, quad: TeQuad, chars: Vec<[f32; 2]>) -> OcrLine {
        OcrLine {
            text: text.into(),
            quad,
            confidence: 1.0,
            chars,
        }
    }

    pub fn read() -> Read {
        Read {
            lines: vec![row(OLD, LINE, chars())],
            blocked: None,
            locked: Vec::new(),
            engine: true,
            failed: false,
        }
    }

    pub fn live_with(r: Read) -> Arc<Live> {
        let l = unread();
        set_read(&l, r);
        l
    }

    pub fn live() -> Arc<Live> {
        live_with(read())
    }

    /// 还没读字的
    pub fn unread() -> Arc<Live> {
        Arc::new(Live::fresh(scene(), None))
    }

    pub fn set_read(l: &Live, r: Read) {
        assert!(l.read.set(r).is_ok());
    }

    pub fn start(l: &Live, ocr: Ocr) {
        l.start(ocr);
    }

    /// 第 `from`..`to` 个字的范围
    pub fn range(from: usize, to: usize) -> [f32; 2] {
        let s = xpaste_pro::te_strip(&scene(), LINE);
        xpaste_pro::te_range(&s, 6, &chars(), from, to)
    }

    pub fn draft(new: &str, range: [f32; 2]) -> Draft {
        Draft {
            edit: None,
            line: LINE,
            range: Some(range),
            old: OLD.into(),
            new: new.into(),
            align: Align::Left,
        }
    }

    /// 页面重排好的一条：第 `k` 个字的格子里涂上 `c`
    fn patched(live: &Live, k: usize, c: [u8; 3]) -> (TeQuad, Vec<u8>) {
        let (h, mut px) = strip(live, &LineRef::Quad(LINE), Extend::default()).unwrap();
        for y in 10..50 {
            for x in k * 60 + 8..k * 60 + 40 {
                let i = (y * h.w as usize + x) * 4;
                px[i..i + 3].copy_from_slice(&c);
            }
        }
        (h.quad, px)
    }

    pub fn put_local(
        t: Tasks,
        dir: &Path,
        live: &Live,
        d: &Draft,
        k: usize,
        c: [u8; 3],
    ) -> Result<u64, AppError> {
        let (q, px) = patched(live, k, c);
        local_patch(t, dir, live, d, &q, &px)
    }

    pub fn edit_of(live: &Live, id: u64) -> Edit {
        let v = live.view();
        v.edits.into_iter().find(|e| e.id == id).unwrap()
    }

    /// 画出来的图上 (x, y) 的颜色
    pub fn at(live: &Live, dir: &Path, x: u32, y: u32) -> [u8; 3] {
        let img = image::load_from_memory(&live.render(dir).unwrap()).unwrap();
        let [r, g, b, _] = img.to_rgba8().get_pixel(x, y).0;
        [r, g, b]
    }

    pub fn msg<T>(r: Result<T, AppError>) -> String {
        r.err().expect("应该拒绝").to_string()
    }

    const RED: [u8; 3] = [255, 0, 0];
    const BLUE: [u8; 3] = [0, 0, 255];

    // ---- 读字 ----
    /// 图先给页面、读字随后到：没读完的等着，读完才有结果
    #[test]
    fn read_is_awaited_once() {
        static N: AtomicU64 = AtomicU64::new(0);
        fn slow(_: &[u8]) -> Result<Option<Vec<OcrLine>>, OcrError> {
            N.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(80));
            Ok(Some(vec![row("甲", LINE, vec![])]))
        }
        let l = Arc::new(Live::fresh(scene(), None));
        assert!(l.read.get().is_none());
        let v = l.view();
        assert_eq!((v.w, v.h, v.edits.len()), (640, 200, 0));
        let other = l.clone();
        let h = std::thread::spawn(move || other.start(slow));
        assert_eq!(l.read().lines[0].text, "甲");
        l.start(slow);
        h.join().unwrap();
        assert_eq!(N.load(Ordering::SeqCst), 1);
    }

    /// 没有引擎、识别出错都不报错；系统解不开这张图的不许改
    #[test]
    fn read_without_engine() {
        let r = Read::new(&scene(), |_| Ok(None));
        assert!(!r.engine && r.lines.is_empty() && r.blocked.is_none() && !r.failed);
        let broken: [Ocr; 2] = [
            |_| Err(OcrError::Timeout),
            |_| Err(OcrError::Engine("x".into())),
        ];
        for ocr in broken {
            let r = Read::new(&scene(), ocr);
            assert!(r.engine && r.lines.is_empty() && !r.failed);
        }
        let r = Read::new(&scene(), |_| Err(OcrError::Decode));
        assert!(r.lines.is_empty() && r.failed);
        // 像素对不上，编码不了
        let mut bad = scene();
        bad.px.pop();
        assert!(Read::new(&bad, |_| Ok(None)).failed);
    }

    // ---- 几何 ----

    const UNIT: TeQuad = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];

    fn moved(q: &TeQuad, dx: f32, dy: f32) -> TeQuad {
        q.map(|p| [p[0] + dx, p[1] + dy])
    }

    #[test]
    fn overlap_and_clash() {
        assert!((overlap(&UNIT, &UNIT) - 100.0).abs() < 1e-6);
        assert!((overlap(&UNIT, &moved(&UNIT, 6.0, 5.0)) - 20.0).abs() < 1e-6);
        assert_eq!(overlap(&UNIT, &moved(&UNIT, 10.0, 0.0)), 0.0);
        // 绕的方向反过来也一样
        let back = [UNIT[1], UNIT[0], UNIT[3], UNIT[2]];
        assert!((overlap(&UNIT, &back) - 100.0).abs() < 1e-6);
        assert!(clash(&UNIT, &moved(&UNIT, 8.0, 0.0)));
        assert!(!clash(&UNIT, &moved(&UNIT, 9.5, 0.0)));
        // 占小的那个的一成以上就算
        let wide = [[0.0, 0.0], [100.0, 0.0], [100.0, 10.0], [0.0, 10.0]];
        assert!(clash(&wide, &moved(&UNIT, 98.0, 0.0)));
        assert!(!clash(&UNIT, &[[5.0; 2]; 4]));
        // 行距紧的上下两行，框叠上一点：不算。框得很高、把另一处整个装进去的算
        assert!(!clash(&UNIT, &moved(&UNIT, 0.0, 8.6)));
        assert!(clash(&UNIT, &moved(&UNIT, 0.0, 6.0)));
        let tall = [[0.0, 0.0], [10.0, 0.0], [10.0, 60.0], [0.0, 60.0]];
        assert!(clash(&tall, &UNIT) && clash(&UNIT, &tall));
    }

    #[test]
    fn signature_clipped_to_image() {
        let q = [[50.0, 10.0], [1e5, 10.0], [1e5, 30.0], [50.0, 30.0]];
        assert_eq!(
            clip(&q, 200, 100),
            [[50.0, 10.0], [200.0, 10.0], [200.0, 30.0], [50.0, 30.0]]
        );
        // 往右下斜的，先碰到下边
        let q = [[0.0, 0.0], [1000.0, 1000.0], [1000.0, 1010.0], [0.0, 10.0]];
        let c = clip(&q, 200, 100);
        assert_eq!((c[1], c[2]), ([100.0, 100.0], [90.0, 100.0]));
    }

    /// 范围换回原图的四角，伸出条带的也对
    #[test]
    fn span_maps_back() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let s = xpaste_pro::te_strip(&scene(), LINE);
        let k = s.img.w as f32 / 352.0;
        let q = span(&s, [60.0 * k, 400.0 * k]);
        for (p, want) in
            q.iter()
                .zip([[136.0, 64.0], [476.0, 64.0], [476.0, 126.0], [136.0, 126.0]])
        {
            assert!((p[0] - want[0]).abs() < 0.1 && (p[1] - want[1]).abs() < 0.1);
        }
        // 外推后的行装得下，换过去的范围还是图上同一段
        let (line, r) = wide(&scene(), &LINE, [-20.0 * k, 400.0 * k]);
        let e = xpaste_pro::te_strip(&scene(), line);
        assert!(r[0] >= -0.1 && r[1] <= e.img.w as f32 + 0.1, "{r:?}");
        let q = span(&e, r);
        assert!(
            (q[0][0] - 56.0).abs() < 0.5 && (q[1][0] - 476.0).abs() < 0.5,
            "{q:?}"
        );
        assert_eq!(wide(&scene(), &LINE, [10.0, 20.0]), (LINE, [10.0, 20.0]));
    }

    #[test]
    fn strip_extends_both_sides() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let l = live();
        let (h, px) = strip(&l, &LineRef::At(0), Extend::default()).unwrap();
        assert_eq!((h.w, h.h, h.left, h.right), (352, 62, 0.0, 0.0));
        assert_eq!(px.len(), 352 * 62 * 4);
        let (h, px) = strip(
            &l,
            &LineRef::Quad(LINE),
            Extend {
                left: 30.0,
                right: 50.0,
            },
        )
        .unwrap();
        assert!((h.left - 30.0).abs() < 1.1 && (h.right - 50.0).abs() < 1.1);
        assert_eq!(px.len(), (h.w * h.h * 4) as usize);
        assert!((h.quad[0][0] - 46.0).abs() < 1.1 && (h.quad[1][0] - 478.0).abs() < 1.1);
        // 截在图内，告诉页面实际推了多少
        let (h, _) = strip(
            &l,
            &LineRef::Quad(LINE),
            Extend {
                left: 500.0,
                right: 500.0,
            },
        )
        .unwrap();
        assert!(
            (h.left - 76.0).abs() < 1.1 && (h.right - 212.0).abs() < 1.1,
            "{} {}",
            h.left,
            h.right
        );
        assert!(strip(&l, &LineRef::At(9), Extend::default()).is_err());
        let nan = LineRef::Quad([[f32::NAN; 2]; 4]);
        assert!(strip(&l, &nan, Extend::default()).is_err());
    }

    /// room 能超出条带；量不出时是条带右端
    #[test]
    fn ranges_have_room() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let l = live();
        let r = ranges(&l, &LineRef::At(0), 6, vec![], 4, 5).unwrap();
        let k = r.w as f32 / 352.0;
        // 第 5 个字占 244–288（行里），右边是第 6 个字
        assert!(r.range[0] < 244.0 * k && r.range[1] > 288.0 * k);
        assert!(
            r.range[0] > 230.0 * k && r.range[1] < 304.0 * k,
            "{:?}",
            r.range
        );
        assert!(r.room >= r.range[1] && r.room < 345.0 * k, "{}", r.room);
        let r = ranges(&l, &LineRef::Quad(LINE), 6, chars(), 6, 6).unwrap();
        assert_eq!(r.range[0], r.range[1]);
        assert!(r.room > r.w as f32 + 50.0, "{}", r.room);
        // 贴着图边的空白行
        let edge = [[600.0, 10.0], [640.0, 10.0], [640.0, 30.0], [600.0, 30.0]];
        let r = ranges(&l, &LineRef::Quad(edge), 2, vec![], 0, 2).unwrap();
        assert_eq!(r.room, r.w as f32);
    }

    // ---- 查 ----

    #[test]
    fn draft_rechecked() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let l = live();
        let ok = draft(NEW, range(4, 5));
        assert!(l.check(&ok).is_ok());
        let bad = |d: Draft| msg(l.check(&d));
        for new in ["", "  ", OLD, "会议改到三点  "] {
            assert_eq!(
                bad(Draft {
                    new: new.into(),
                    ..ok.clone()
                }),
                NOT_CHANGED
            );
        }
        assert_eq!(
            bad(Draft {
                old: " ".into(),
                ..ok.clone()
            }),
            NOT_CHANGED
        );
        // 60 个字位簇，不按码点数
        let long = |n: usize| "👨‍👩‍👧".repeat(n);
        assert!(
            l.check(&Draft {
                new: long(60),
                ..ok.clone()
            })
            .is_ok()
        );
        assert_eq!(
            bad(Draft {
                new: long(61),
                ..ok.clone()
            }),
            TOO_LONG
        );
        assert_eq!(
            bad(Draft {
                old: long(61),
                ..ok.clone()
            }),
            TOO_LONG
        );
        for r in [
            None,
            Some([f32::NAN, 1.0]),
            Some([9.0, 3.0]),
            Some([0.0, f32::INFINITY]),
            Some([9000.0, 9100.0]),
        ] {
            assert_eq!(
                bad(Draft {
                    range: r,
                    ..ok.clone()
                }),
                BAD_RANGE,
                "{r:?}"
            );
        }
        for line in [[[f32::NAN; 2]; 4], moved(&LINE, 5000.0, 5000.0)] {
            let r = l.check(&Draft { line, ..ok.clone() });
            assert_eq!(msg(r), BAD_RANGE);
        }
        // 末尾空格不算
        let s = l
            .check(&Draft {
                new: format!("{NEW}  "),
                ..ok
            })
            .unwrap();
        assert_eq!(s.new, NEW);
        // 读出来就更长的行，不比原文长就行
        let text: String = "字".repeat(70);
        let l = live_with(Read {
            lines: vec![row(&text, LINE, vec![])],
            ..read()
        });
        let d = Draft {
            old: text.clone(),
            new: format!("{}改", "字".repeat(69)),
            ..draft("", range(4, 5))
        };
        assert!(l.check(&d).is_ok());
        assert_eq!(
            msg(l.check(&Draft {
                new: format!("{text}改"),
                ..d
            })),
            TOO_LONG
        );
    }

    /// 命中的图：走法、AI、本机都拒绝，不落盘
    #[test]
    fn blocked_never_changes() {
        let (t, tmp) = (new_tasks(), Tmp::new());
        let l = live_with(Read {
            blocked: Some(TeBlock::Receipt),
            ..read()
        });
        let d = draft(NEW, [0.0, 10.0]);
        assert_eq!(msg(l.plan(&d, false, true)), BLOCKED);
        assert_eq!(msg(run_local(t, &tmp.0, &l, &d)), BLOCKED);
        assert_eq!(msg(local_patch(t, &tmp.0, &l, &d, &LINE, &[0; 8])), BLOCKED);
        assert_eq!(msg(pick(t, &tmp.0, &l, 1, 0)), BLOCKED);
        assert_eq!(std::fs::read_dir(&tmp.0).unwrap().count(), 0);
        assert!(l.view().edits.is_empty());
    }

    /// 碰到签名段的拒绝；紧挨着的上一行、同一行别的字照常
    #[test]
    fn locked_span_rejected() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (new_tasks(), Tmp::new());
        // 从第 5 个字起锁住
        let lock_at = |x: f32, dy: f32| {
            let q = [[x, 64.0], [640.0, 64.0], [640.0, 126.0], [x, 126.0]];
            live_with(Read {
                locked: vec![moved(&q, 0.0, dy)],
                ..read()
            })
        };
        let l = lock_at(314.0, 0.0);
        for r in [range(4, 5), range(5, 6), range(3, 5), [0.0, 4000.0]] {
            let d = draft(NEW, r);
            assert_eq!(msg(l.check(&d)), LOCKED, "{r:?}");
            assert_eq!(msg(l.plan(&d, false, true)), LOCKED);
            assert_eq!(msg(run_local(t, &tmp.0, &l, &d)), LOCKED);
        }
        assert!(l.check(&draft("会议政到三点", range(2, 3))).is_ok());
        // 页面给的范围不碰，真正改的碰到了：也拒绝
        let d = draft("会议政到三点", range(2, 3));
        assert_eq!(msg(put_local(t, &tmp.0, &l, &d, 4, RED)), LOCKED);
        assert!(put_local(t, &tmp.0, &l, &d, 2, RED).is_ok());
        // 说的是这一行，交回来的条带在别的行
        let below = lock_at(76.0, 70.0);
        let (q, px) = {
            let line = LineRef::Quad(moved(&LINE, 0.0, 70.0));
            let (h, mut px) = strip(&below, &line, Extend::default()).unwrap();
            let i = (30 * 352 + 150) * 4;
            px[i..i + 3].copy_from_slice(&RED);
            (h.quad, px)
        };
        assert_eq!(msg(local_patch(t, &tmp.0, &below, &d, &q, &px)), BAD_RANGE);
        // 条带取得很高，把下面的签名也带进来改了
        let both = [LINE[0], LINE[1], [428.0, 190.0], [76.0, 190.0]];
        let (h, mut px) = strip(&below, &LineRef::Quad(both), Extend::default()).unwrap();
        let i = (100 * h.w as usize + 150) * 4;
        px[i..i + 3].copy_from_slice(&RED);
        let d = draft("会议政到三点", range(2, 3));
        assert_eq!(msg(local_patch(t, &tmp.0, &below, &d, &both, &px)), LOCKED);
        // 字数变多、占到签名那段的也拒绝
        assert_eq!(
            msg(l.check(&draft("会议改到三点半钟", range(3, 4)))),
            LOCKED
        );
        // 下一行是签名，框叠上几个像素
        assert!(lock_at(76.0, 56.0).check(&draft(NEW, range(4, 5))).is_ok());
        // 范围没扩之前是空的也查
        assert_eq!(
            msg(lock_at(430.0, 0.0).check(&draft("会议改到三点半", range(6, 6)))),
            LOCKED
        );
    }

    /// 框得很高、签名只占框的上面一截：也拒绝
    #[test]
    fn tall_box_over_signature_rejected() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let sig = [[314.0, 64.0], [640.0, 64.0], [640.0, 88.0], [314.0, 88.0]];
        let l = live_with(Read {
            locked: vec![sig],
            ..read()
        });
        for bottom in [126.0, 190.0] {
            let line = [[76.0, 64.0], [428.0, 64.0], [428.0, bottom], [76.0, bottom]];
            let k = xpaste_pro::te_strip(&scene(), line).img.w as f32 / 352.0;
            let d = |a: f32, b: f32| Draft {
                line,
                ..draft(NEW, [a * k, b * k])
            };
            assert_eq!(msg(l.check(&d(242.0, 296.0))), LOCKED, "{bottom}");
            assert!(l.check(&d(122.0, 176.0)).is_ok(), "{bottom}");
        }
    }

    /// 真正变了的像素在签名里才算；上下边让一点
    #[test]
    fn spoiled_pixels() {
        let sig = [[300.0, 60.0], [400.0, 60.0], [400.0, 100.0], [300.0, 100.0]];
        let l = live_with(Read {
            locked: vec![sig],
            ..read()
        });
        let hits = |at: Option<(u32, u32)>| {
            let mut patch = cut(&l.img, [280, 50, 420, 110]);
            if let Some((x, y)) = at {
                patch.px[(((y - 50) * 140 + x - 280) * 4) as usize] ^= 0x40;
            }
            l.spoils_locked(&TePaste {
                patch,
                x: 280,
                y: 50,
                exact: true,
                soft: false,
            })
        };
        assert!(!hits(None) && !hits(Some((299, 80))) && !hits(Some((350, 61))));
        assert!(hits(Some((300, 80))) && hits(Some((399, 65))));
    }

    /// 给不给改没判断成：什么都不许改
    #[test]
    fn no_reading_no_editing() {
        let (t, tmp) = (new_tasks(), Tmp::new());
        let l = unread();
        l.no_reading();
        let d = draft(NEW, [0.0, 10.0]);
        assert_eq!(msg(l.check(&d)), UNREADABLE);
        assert_eq!(msg(l.plan(&d, false, true)), UNREADABLE);
        assert_eq!(
            msg(local_patch(t, &tmp.0, &l, &d, &LINE, &[0; 8])),
            UNREADABLE
        );
        assert_eq!(msg(pick(t, &tmp.0, &l, 1, 0)), UNREADABLE);
        assert!(!l.read().engine);
        // 晚到的读字结果不改主意
        l.start(|_| Ok(Some(vec![])));
        assert_eq!(msg(l.check(&d)), UNREADABLE);
    }

    #[test]
    fn counts_changed_chars() {
        for (old, new, n) in [
            (OLD, NEW, 1),
            (OLD, "会议改到三点半", 1),
            (OLD, "会议改到", 2),
            ("aa", "aaa", 1),
            ("甲乙", "丙丁戊", 3),
            (OLD, OLD, 0),
        ] {
            assert_eq!(changed_count(old, new), n, "{old} {new}");
        }
    }

    // ---- 范围 ----

    #[test]
    fn range_grows_only_for_more_text() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let l = live();
        let (r, end) = (range(4, 5), range(6, 6));
        assert_eq!(l.check(&draft(NEW, r)).unwrap().range, r);
        assert_eq!(
            l.check(&draft("会议改到", range(4, 6))).unwrap().range,
            range(4, 6)
        );
        // 加两个字：右边多两个半字宽（一个字 58.7）
        let g = l.check(&draft("会议改到三点一刻", end)).unwrap().range;
        let k = xpaste_pro::te_strip(&scene(), LINE).img.w as f32 / 6.0;
        assert_eq!(g[0], end[0]);
        assert!((g[1] - g[0] - 2.5 * k).abs() < 1.0, "{g:?}");
        // 居中两边都占，靠右占左边，都不出图
        let d = |align| Draft {
            align,
            ..draft("会议改到三点一刻", range(0, 6))
        };
        let (c, rt) = (
            l.check(&d(Align::Center)).unwrap().range,
            l.check(&d(Align::Right)).unwrap().range,
        );
        assert!(c[0] < range(0, 6)[0] && c[1] > range(0, 6)[1], "{c:?}");
        assert!(rt[0] < c[0] && rt[1] == range(0, 6)[1], "{rt:?}");
        assert!(rt[0] >= -76.5 * xpaste_pro::te_strip(&scene(), LINE).img.w as f32 / 352.0);
    }

    // ---- 走法 ----

    #[test]
    fn plan_modes() {
        use TeVerdict::{Bad, Pass, Soft};
        let cases: [(Option<TeVerdict>, [&str; 2], Option<bool>); 4] = [
            (Some(Pass), ["local", "ai"], Some(false)),
            (Some(Soft), ["soft", "ai"], Some(true)),
            (Some(Bad), ["ai", "ai"], None),
            (None, ["ai", "ai"], None),
        ];
        for (local, ways, soft) in cases {
            for (ai, way) in [false, true].into_iter().zip(ways) {
                for api in [false, true] {
                    let p = plan_of(ai, api, local.map(|v| (v, "有残影")));
                    let mut could = vec![];
                    if soft.is_some() {
                        could.push("local");
                    }
                    if api {
                        could.push("ai");
                    }
                    assert_eq!(
                        (p.way, p.soft, p.could),
                        (way, soft, could),
                        "{local:?} {ai} {api}"
                    );
                    assert_eq!(p.why, if local.is_some() { "有残影" } else { "" });
                }
            }
        }
        let v = serde_json::to_value(plan_of(false, false, None)).unwrap();
        assert!(v.get("soft").is_none());
    }

    /// 删末尾的字本机能做；算过的不重算，提交直接用；删中间的、居中的不行
    #[test]
    fn local_erase_places() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (new_tasks(), Tmp::new());
        let l = live();
        let d = draft("会议改到三", range(5, 6));
        let p = l.plan(&d, false, false).unwrap();
        assert_eq!(
            (p.way, &p.could[..]),
            ("local", &["local"][..]),
            "{}",
            p.why
        );
        let first = l.local(&l.check(&d).unwrap()).unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &l.local(&l.check(&d).unwrap()).unwrap()
        ));
        let id = run_local(t, &tmp.0, &l, &d).unwrap();
        let e = edit_of(&l, id);
        assert_eq!(
            (e.pick, e.cands.len(), e.cands[0].by == store::By::Local),
            (0, 1, true)
        );
        // 第 6 个字没了，第 5 个字没动
        assert!(at(&l, &tmp.0, 80 + 300 + 38, 90)[0] > 200);
        assert_eq!(at(&l, &tmp.0, 80 + 240 + 30, 90), [20, 24, 30]);
        assert!(l.view().rev > 0);
        for d in [
            draft("会议改到点", range(4, 6)),
            Draft {
                align: Align::Center,
                ..d
            },
            draft("会议改到三点半", range(6, 6)),
        ] {
            assert_eq!(l.plan(&d, false, true).unwrap().way, "ai");
            assert_eq!(msg(run_local(t, &tmp.0, &l, &d)), NO_LOCAL);
        }
    }

    /// 搬两个字：同一张图里的另一行有
    #[test]
    fn local_move_two_chars() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (new_tasks(), Tmp::new());
        let l = live();
        // 第 5、6 个字换成第 1、2 个字
        let d = draft("会议改到会议", range(4, 6));
        let p = l.plan(&d, false, false).unwrap();
        assert_ne!(p.way, "ai", "{}", p.why);
        let id = run_local(t, &tmp.0, &l, &d).unwrap();
        assert!(edit_of(&l, id).cands[0].placed);
        // 第 1 个字的一竖在最左、一横在最上
        assert!(at(&l, &tmp.0, 80 + 240 + 4, 100)[0] < 60);
        assert!(at(&l, &tmp.0, 80 + 240 + 32, 100)[0] > 200);
        // 来源没动
        assert_eq!(at(&l, &tmp.0, 84, 100), [20, 24, 30]);
        // 拖窄了的范围只到第 6 个字：够不着第 5 个
        let narrow = draft("会议改到会议", range(5, 6));
        assert_eq!(l.plan(&narrow, false, true).unwrap().way, "ai");
        let s = xpaste_pro::te_strip(&scene(), LINE);
        let get = |at: &[usize], r| targets(&s, 6, &chars(), at, r);
        let two = get(&[1, 4], [range(1, 2)[0] - 3.0, range(4, 5)[1]]).unwrap();
        assert_eq!(two[0], [range(1, 2)[0] - 3.0, range(1, 2)[1]]);
        assert_eq!(two[1], range(4, 5));
        // 中间的边也不超出
        assert!(get(&[4, 5], range(5, 6)).is_none() && get(&[4, 5], range(4, 5)).is_none());
        // 签名里的不搬
        let d = draft("会会改到三点", range(1, 2));
        assert_ne!(l.plan(&d, false, true).unwrap().way, "ai");
        let signed = [[76.0, 64.0], [124.0, 64.0], [124.0, 126.0], [76.0, 126.0]];
        let l = live_with(Read {
            locked: vec![signed],
            ..read()
        });
        assert_eq!(l.plan(&d, false, true).unwrap().way, "ai");
    }

    /// 搬字的来源里没有和要改的这行是同一行的
    #[test]
    fn own_line_not_a_source() {
        let far = [[76.0, 140.0], [428.0, 140.0], [428.0, 196.0], [76.0, 196.0]];
        let l = live_with(Read {
            lines: vec![row(OLD, LINE, chars()), row("别的行", far, vec![])],
            ..read()
        });
        for (line, chars) in [(LINE, 6), (moved(&LINE, 3.0, -4.0), 0)] {
            let d = Draft {
                line,
                ..draft(NEW, [0.0, 1.0])
            };
            let rows = l.rows(&spot(&d, [0.0, 1.0]), 6);
            assert_eq!(rows.len(), 2);
            assert!(rows[0].1 == far && rows[1].1 == line && rows[1].2.len() == chars);
        }
        // 用户改过原文，字数对不上：逐字框不能用
        let d = draft(NEW, [0.0, 1.0]);
        assert!(l.rows(&spot(&d, [0.0, 1.0]), 5)[1].2.is_empty());
    }

    // ---- 替换 ----

    /// 同一个地方接连改两次：只画后一处，前一处的候选留着
    #[test]
    fn same_line_submits_replace() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (new_tasks(), Tmp::new());
        let l = live();
        let d = draft(NEW, range(4, 5));
        let a = put_local(t, &tmp.0, &l, &d, 4, RED).unwrap();
        assert_eq!(at(&l, &tmp.0, 76 + 260, 90), RED);
        let rev = l.view().rev;
        let b = put_local(t, &tmp.0, &l, &draft("会议改到五点", range(4, 5)), 4, BLUE).unwrap();
        assert!(l.view().rev > rev);
        assert_eq!((edit_of(&l, a).pick, edit_of(&l, b).pick), (NO_PICK, 0));
        assert_eq!(edit_of(&l, a).cands.len(), 1);
        assert_eq!(at(&l, &tmp.0, 76 + 260, 90), BLUE);
        // AI 的也一样：提交时就取消选中，在改的取消、没改出来的去掉
        let (first, c) = t.submit(&tmp.0, &l, &d, true, true).unwrap();
        assert_eq!(edit_of(&l, b).pick, NO_PICK);
        let (second, e) = t
            .submit(&tmp.0, &l, &draft("会议改到六点", range(4, 5)), true, true)
            .unwrap();
        assert_ne!(c, e);
        let ids: Vec<u64> = t.views().iter().map(|v| v.id).collect();
        assert!(!ids.contains(&first) && ids.contains(&second));
        // 存在盘上的也是这样
        let back = store::load(&tmp.0, &l.id).unwrap();
        assert!(back.edits.iter().all(|e| e.pick == NO_PICK));
    }

    /// 带了从哪一处打开的：范围不相交也替换；字和范围没变的是同一处再来一张
    #[test]
    fn reopened_edit_replaced() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (new_tasks(), Tmp::new());
        let l = live();
        let d = draft(NEW, range(4, 5));
        let a = put_local(t, &tmp.0, &l, &d, 4, RED).unwrap();
        let e = edit_of(&l, a);
        let same = Draft {
            edit: Some(a),
            range: Some(e.range),
            ..d.clone()
        };
        let (task, again) = t.submit(&tmp.0, &l, &same, true, true).unwrap();
        assert_eq!((again, edit_of(&l, a).pick), (a, 0));
        t.cancel(task);
        let moved = Draft {
            edit: Some(a),
            ..draft("会政改到三点", range(1, 2))
        };
        let b = put_local(t, &tmp.0, &l, &moved, 1, BLUE).unwrap();
        assert_ne!(a, b);
        assert_eq!((edit_of(&l, a).pick, edit_of(&l, b).pick), (NO_PICK, 0));
        let gone = Draft {
            edit: Some(99),
            ..moved
        };
        assert_eq!(msg(put_local(t, &tmp.0, &l, &gone, 1, RED)), NO_EDIT);
    }

    /// 对齐跟着那一处存：读回来照它再出一张是同一处，换了对齐的是另一处
    #[test]
    fn align_kept() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (new_tasks(), Tmp::new());
        let l = live();
        let d = Draft {
            align: Align::Center,
            ..draft("会议改到三点一刻", range(0, 6))
        };
        let (task, id) = t.submit(&tmp.0, &l, &d, true, true).unwrap();
        t.cancel(task);
        let back = store::load(&tmp.0, &l.id).unwrap();
        let e = back.edits.into_iter().find(|e| e.id == id).unwrap();
        assert!(e.align == Align::Center);
        let more = Draft {
            edit: Some(id),
            range: Some(e.range),
            align: e.align,
            ..d
        };
        let s = l.check(&more).unwrap();
        assert_eq!((s.range, s.realign()), (e.range, Some("居中")));
        let (task, same) = t.submit(&tmp.0, &l, &more, true, true).unwrap();
        assert_eq!(same, id);
        t.cancel(task);
        let right = Draft {
            align: Align::Right,
            ..more
        };
        let (task, other) = t.submit(&tmp.0, &l, &right, true, true).unwrap();
        assert_ne!(other, id);
        assert!(edit_of(&l, other).align == Align::Right);
        t.cancel(task);
        // 字数没变的用不着对齐：还是那一处
        let a = put_local(t, &tmp.0, &l, &draft(NEW, range(4, 5)), 4, RED).unwrap();
        let e = edit_of(&l, a);
        let again = Draft {
            edit: Some(a),
            range: Some(e.range),
            align: Align::Right,
            ..draft(NEW, range(4, 5))
        };
        let (_, same) = t.submit(&tmp.0, &l, &again, true, true).unwrap();
        assert_eq!(same, a);
    }

    #[test]
    fn disjoint_ranges_on_same_line_both_kept() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (new_tasks(), Tmp::new());
        let l = live();
        let a = put_local(t, &tmp.0, &l, &draft("会政改到三点", range(1, 2)), 1, RED).unwrap();
        let b = put_local(t, &tmp.0, &l, &draft("会议改到四点", range(4, 5)), 4, BLUE).unwrap();
        assert_eq!((edit_of(&l, a).pick, edit_of(&l, b).pick), (0, 0));
        assert_eq!(at(&l, &tmp.0, 76 + 80, 90), RED);
        assert_eq!(at(&l, &tmp.0, 76 + 260, 90), BLUE);
        // 挨着的两个字也不算
        let c = put_local(t, &tmp.0, &l, &draft("会议政到三点", range(2, 3)), 2, RED).unwrap();
        assert_eq!((edit_of(&l, a).pick, edit_of(&l, c).pick), (0, 0));
    }

    #[test]
    fn insert_at_end_twice_replaces() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (new_tasks(), Tmp::new());
        let l = live();
        let end = range(6, 6);
        assert_eq!(end[0], end[1]);
        let d = |new| draft(new, end);
        let (first, a) = t
            .submit(&tmp.0, &l, &d("会议改到三点半"), true, true)
            .unwrap();
        let (second, b) = t
            .submit(&tmp.0, &l, &d("会议改到三点一刻"), true, true)
            .unwrap();
        assert_ne!(a, b);
        assert!(t.views().iter().all(|v| v.id != first));
        t.cancel(second);
        // 再出一张：范围已经扩过，不再变宽，还是那一处
        let more = Draft {
            edit: Some(b),
            range: Some(edit_of(&l, b).range),
            ..d("会议改到三点一刻")
        };
        let (_, same) = t.submit(&tmp.0, &l, &more, true, true).unwrap();
        assert_eq!(same, b);
        // 在别处加的不相干
        let (other, _) = t
            .submit(&tmp.0, &l, &draft("会政改到三点", range(1, 2)), true, true)
            .unwrap();
        assert!(t.views().iter().any(|v| v.id == other) && t.views().len() == 2);
    }

    /// 选回前一处的候选：后一处取消选中，它在改的也取消
    #[test]
    fn pick_back_deselects_same_line() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (new_tasks(), Tmp::new());
        let l = live();
        let a = put_local(t, &tmp.0, &l, &draft(NEW, range(4, 5)), 4, RED).unwrap();
        let d = draft("会议改到五点", range(4, 5));
        let b = put_local(t, &tmp.0, &l, &d, 4, BLUE).unwrap();
        let e = edit_of(&l, b);
        let more = Draft {
            edit: Some(b),
            range: Some(e.range),
            ..d
        };
        let (task, same) = t.submit(&tmp.0, &l, &more, true, true).unwrap();
        assert_eq!(same, b);
        pick(t, &tmp.0, &l, a, 0).unwrap();
        assert_eq!((edit_of(&l, a).pick, edit_of(&l, b).pick), (0, NO_PICK));
        assert!(t.views().iter().all(|v| v.id != task));
        assert_eq!(at(&l, &tmp.0, 76 + 260, 90), RED);
        assert!(pick(t, &tmp.0, &l, a, 1).is_err());
        assert!(pick(t, &tmp.0, &l, 9, 0).is_err());
        // 改回去
        undo(t, &tmp.0, &l, a).unwrap();
        assert_eq!(edit_of(&l, a).cands.len(), 1);
        assert_ne!(at(&l, &tmp.0, 76 + 260, 90), RED);
        assert!(undo(t, &tmp.0, &l, 9).is_err());
    }

    /// 没存进去的：文件夹刚没的再等一回，撤销回来就补存；还没有就放开
    #[test]
    fn kept_until_undo_is_over() {
        let (t, tmp) = (new_tasks(), Tmp::new());
        for back in [true, false] {
            let l = unread();
            l.commit(t, &tmp.0, |job| {
                put(job, &spot(&draft(NEW, [0.0, 1.0]), [0.0, 1.0]))?;
                store::save(&tmp.0, job).map_err(not_saved)
            })
            .unwrap();
            lock(&l.st).late.push(Late {
                task: 1,
                edit: 1,
                paste: TePaste {
                    patch: cut(&l.img, [0, 0, 2, 2]),
                    x: 0,
                    y: 0,
                    exact: true,
                    soft: false,
                },
                note: "",
                placed: true,
                pick: true,
            });
            let at = Instant::now();
            lock(&KEPT).push(Kept {
                live: l.clone(),
                at,
            });
            let kept = || lock(&KEPT).iter().any(|k| k.live.id == l.id);
            let (dir, aside) = (tmp.0.join(&l.id), tmp.0.join("aside"));
            std::fs::rename(&dir, &aside).unwrap();
            let mut missing = false;
            assert!(tend(t, &tmp.0, &l.id, at, &mut missing) && kept());
            if back {
                std::fs::rename(&aside, &dir).unwrap();
                assert!(tend(t, &tmp.0, &l.id, at, &mut missing) && !kept());
                assert_eq!((edit_of(&l, 1).pick, edit_of(&l, 1).cands.len()), (0, 1));
            }
            assert!(!tend(t, &tmp.0, &l.id, at, &mut missing) && !kept());
        }
    }

    // ---- 本机重排交回的 ----

    /// 实际改的比页面给的宽：范围按实际的；没改的字节不变
    #[test]
    fn patch_range_is_real() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (new_tasks(), Tmp::new());
        let l = live();
        let (q, mut px) = patched(&l, 4, RED);
        // 第 6 个字也动了
        let i = (30 * 352 + 320) * 4;
        px[i..i + 3].copy_from_slice(&RED);
        let d = draft(NEW, range(4, 5));
        let id = local_patch(t, &tmp.0, &l, &d, &q, &px).unwrap();
        let k = xpaste_pro::te_strip(&scene(), LINE).img.w as f32 / 352.0;
        let r = edit_of(&l, id).range;
        assert!(
            r[0] == range(4, 5)[0] && (r[1] - 321.0 * k).abs() < 0.5,
            "{r:?}"
        );
        let out = image::load_from_memory(&l.render(&tmp.0).unwrap())
            .unwrap()
            .into_rgba8();
        let orig = scene();
        for (i, (a, b)) in out.as_raw().chunks(4).zip(orig.px.chunks(4)).enumerate() {
            let (x, y) = (i % 640, i / 640);
            let hit = ((76 + 248..76 + 280).contains(&x) && (64 + 10..64 + 50).contains(&y))
                || (x, y) == (76 + 320, 64 + 30);
            assert_eq!(a == b, !hit, "{x} {y}");
        }
        // 尺寸不对、没有变化的拒绝
        assert_eq!(msg(local_patch(t, &tmp.0, &l, &d, &q, &px[4..])), BAD_RANGE);
        let (_, same) = strip(&l, &LineRef::Quad(LINE), Extend::default()).unwrap();
        assert_eq!(msg(local_patch(t, &tmp.0, &l, &d, &q, &same)), NOT_CHANGED);
    }

    // ---- 留存 ----
}

#[cfg(test)]
#[path = "textedit_gates.rs"]
mod gates;
