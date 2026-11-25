//! 改字：打开一张图，和改字窗口的运行时（只在带 Pro 的构建里启用）。一张图里的事见 `textedit_job`，AI 任务见 `textedit_tasks`。
//!
//! 窗口只拿着当前那张图，没结束的任务各自拿着自己那张：换图、关窗口都不影响它们。

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use image::{DynamicImage, ImageDecoder, ImageReader};
use serde::Serialize;
use tauri::{AppHandle, Manager};
use xpaste_pro::TeImg;

use crate::error::AppError;
use crate::events::{self, emit_to};
use crate::meme::{self, Entry, Origin};
use crate::panel::{Rect, on_main};
use crate::state::AppState;
use crate::textedit_job::{self as job, GONE, JobView, Live, lock, no, not_saved};
use crate::textedit_store as store;
use crate::textedit_tasks::{Tasks, tasks};
use crate::textedit_window;

pub const LABEL: &str = "textedit";
/// 图最多多少像素
const MAX_PX: u64 = 100_000_000;
/// 打开的文件最多多大：再大也是读不了的
const MAX_BYTES: u64 = 1 << 30;

const OFF: &str = "改字没有开启";
/// 一次给了多个文件，只用一个
const ONE: &str = "一次改一张";
const EXPIRED: &str = "试用期已结束，改字需要授权";
const UNUSABLE: &str = "这条改不了字";

// ---- 打开 ----

/// 从哪儿来。`Shot` 的图是刚截的，`rect` 是它在屏幕上的位置（原生单位，交给 `shot::pin_at`）
pub enum Source {
    Shot {
        #[allow(dead_code)] // “钉回原处”接上后删
        rect: Rect,
        png: Vec<u8>,
    },
    Clip {
        record: i64,
    },
    Panel {
        record: i64,
        file: Option<u32>,
    },
    Pin {
        n: u64,
    },
    File(PathBuf),
    Blank,
    Recent(String),
}

impl Source {
    /// 给页面和日志的种类
    pub fn kind(&self) -> &'static str {
        match self {
            Source::Shot { .. } => "shot",
            Source::Clip { .. } => "clip",
            Source::Panel { .. } => "panel",
            Source::Pin { .. } => "pin",
            Source::File(_) => "file",
            Source::Blank => "blank",
            Source::Recent(_) => "recent",
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum OpenFail {
    Unreadable,
    TooBig,
}

impl OpenFail {
    pub fn text(&self) -> &'static str {
        match self {
            OpenFail::Unreadable => job::UNREADABLE,
            OpenFail::TooBig => "图太大，改不了",
        }
    }
}

/// 解码：按 EXIF 摆正，动图取第一帧；先看头部的尺寸再解。第二项是 RGB 的 ICC，灰度、CMYK 的图解成 RGB 后不能再带原来的
pub fn decode(bytes: &[u8]) -> Result<(TeImg, Option<Vec<u8>>), OpenFail> {
    let bad = |_| OpenFail::Unreadable;
    let mut r = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| OpenFail::Unreadable)?;
    // 1 亿像素的 16 位图要 800 MB，默认的上限装不下
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(1 << 30);
    r.limits(limits);
    let mut d = r.into_decoder().map_err(bad)?;
    let (w, h) = d.dimensions();
    if u64::from(w) * u64::from(h) > MAX_PX {
        return Err(OpenFail::TooBig);
    }
    let turn = d
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let icc = d.icc_profile().ok().flatten();
    let icc = icc.filter(|p| p.get(16..20) == Some(b"RGB "));
    let mut img = DynamicImage::from_decoder(d).map_err(bad)?;
    img.apply_orientation(turn);
    let img = img.into_rgba8();
    let img = TeImg {
        w: img.width(),
        h: img.height(),
        px: img.into_raw(),
    };
    Ok((img, icc))
}

/// 图片文件摆正后的宽高，只读文件头
pub fn dims(path: &Path) -> Option<(u32, u32)> {
    let r = ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let mut d = r.into_decoder().ok()?;
    let (w, h) = d.dimensions();
    use image::metadata::Orientation::*;
    Some(match d.orientation().unwrap_or(NoTransforms) {
        Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH => (h, w),
        _ => (w, h),
    })
}

/// 来源的图片文件字节
fn bytes_of(state: &AppState, src: &mut Source) -> Result<Vec<u8>, AppError> {
    let unreadable = || no(OpenFail::Unreadable.text());
    let path = match src {
        Source::Shot { png, .. } => return Ok(std::mem::take(png)),
        Source::Clip { record } => record_path(state, *record, None)?,
        Source::Panel { record, file } => record_path(state, *record, *file)?,
        Source::Pin { n } => state.data_dir.join("pins").join(format!("{n}.png")),
        Source::File(p) => p.clone(),
        Source::Blank | Source::Recent(_) => return Err(unreadable()),
    };
    // 只收普通文件：管道、设备读起来没有头
    let m = std::fs::metadata(&path).map_err(|_| unreadable())?;
    if !m.is_file() {
        return Err(unreadable());
    }
    if m.len() > MAX_BYTES {
        return Err(no(OpenFail::TooBig.text()));
    }
    std::fs::read(&path).map_err(|_| unreadable())
}

/// 历史记录的图；没说第几个文件的，文件记录取第一个图片文件
fn record_path(state: &AppState, id: i64, file: Option<u32>) -> Result<PathBuf, AppError> {
    let file = match file {
        Some(_) => file,
        None => {
            let first = meme::record_refs(state, id).into_iter().next();
            first.ok_or_else(|| no(UNUSABLE))?.file
        }
    };
    crate::record_image::path_of(state, id, file).ok_or_else(|| no(OpenFail::Unreadable.text()))
}

// ---- 窗口的运行时 ----

/// 发给页面的 `TE_OPEN`。空白窗口、试用结束时没有图；里面有改过的字，不派生 Debug
#[derive(Clone, Serialize)]
pub struct OpenPayload {
    pub job: Option<JobView>,
    pub from: &'static str,
    /// 粘贴目标的应用名
    pub target: Option<String>,
    pub expired: bool,
    #[serde(flatten)]
    pub config: Config,
}

/// 设置的“怎么改”，生图接口配好没有
#[derive(Clone, Serialize)]
pub struct Config {
    pub mode: String,
    pub api: bool,
}

/// 一次打开：载荷，和打开后的提示
type Opening = (OpenPayload, Option<&'static str>);

struct Runtime {
    open: bool,
    /// 页面把事件都听上了
    ready: bool,
    /// 听上之前的最后一次打开
    held: Option<Opening>,
    origin: Origin,
    cur: Option<Arc<Live>>,
    /// 窗口关着时改好了几处
    unseen: u32,
    /// 上屏的（或作废的）最大的打开号
    shown: u64,
}

/// 这个号还轮得到上屏：比上过屏的新
fn admit(shown: &mut u64, seq: u64) -> bool {
    let ok = seq > *shown;
    *shown = seq.max(*shown);
    ok
}

impl Runtime {
    const fn new() -> Self {
        Runtime {
            open: false,
            ready: false,
            held: None,
            origin: meme::NOWHERE,
            cur: None,
            unseen: 0,
            shown: 0,
        }
    }

    /// 页面就绪了就交回去发，没有就先留着
    fn deliver(&mut self, o: Opening) -> Option<Opening> {
        if self.ready {
            return Some(o);
        }
        self.held = Some(o);
        None
    }

    /// 页面就绪：要补发的那次打开
    fn page_ready(&mut self) -> Option<Opening> {
        self.ready = true;
        self.held.take()
    }
}

static RT: Mutex<Runtime> = Mutex::new(Runtime::new());

/// 还有人拿着的图：窗口里的、有任务没结束的。同一张图只有一份
static LIVE: Mutex<Vec<Weak<Live>>> = Mutex::new(Vec::new());

/// 打开的号：先开的图解得慢、后开的已经上屏了，就不换回去。打不开的不算
static OPENS: AtomicU64 = AtomicU64::new(0);

pub fn dir(app: &AppHandle) -> PathBuf {
    app.state::<AppState>().data_dir.join("textedit")
}

/// 编入了、并且设置里开着
pub fn on(app: &AppHandle) -> bool {
    crate::hotkey::edit_enabled() && app.state::<AppState>().settings().edit_enabled
}

fn expired(app: &AppHandle) -> bool {
    !crate::mixer::accepts_new(&crate::mixer::pro(app))
}

/// 现在能不能改；会产出改动的命令都先问它
pub fn editable(app: &AppHandle) -> Result<(), AppError> {
    may_edit(on(app), || expired(app)).map_err(no)
}

/// 没开启时返回原因
pub fn check_on(app: &AppHandle) -> Result<(), AppError> {
    on(app).then_some(()).ok_or_else(|| no(OFF))
}

fn may_edit(on: bool, expired: impl FnOnce() -> bool) -> Result<(), &'static str> {
    match on {
        false => Err(OFF),
        true if expired() => Err(EXPIRED),
        true => Ok(()),
    }
}

pub fn config(app: &AppHandle) -> Config {
    let s = app.state::<AppState>().settings().clone();
    Config {
        api: !crate::imagegen::unconfigured(&crate::meme_gen::image_cfg(&s)),
        mode: s.edit_mode,
    }
}

/// 登记，开始读字；同一张已经有了就用那一份。从盘上读回来的给 `dir`：读图要一会儿，这中间可能有人改完又放手了
fn enlist(mut live: Live, dir: Option<&Path>) -> Arc<Live> {
    let live = {
        let mut all = lock(&LIVE);
        all.retain(|l| l.strong_count() > 0);
        if let Some(l) = all
            .iter()
            .filter_map(Weak::upgrade)
            .find(|l| l.id == live.id)
        {
            return l;
        }
        if let Some(dir) = dir {
            live.reload(dir);
        }
        let live = Arc::new(live);
        all.push(Arc::downgrade(&live));
        live
    };
    let l = live.clone();
    if !crate::housekeeping::spawn("xpaste-textedit-read", move || l.start(job::ocr)) {
        live.no_reading();
    }
    live
}

/// 一张图：还有人拿着的用那一份，否则从“最近改过”读回来。耗时
pub fn find(app: &AppHandle, id: &str) -> Result<Arc<Live>, AppError> {
    if let Some(l) = lock(&LIVE)
        .iter()
        .filter_map(Weak::upgrade)
        .find(|l| l.id == id)
    {
        return Ok(l);
    }
    let dir = dir(app);
    let live = Live::load(&dir, id).map_err(|_| no(GONE))?;
    Ok(enlist(live, Some(&dir)))
}

/// 打开一张图，换掉窗口里的。粘贴目标按这一刻的前台应用记。要读文件、解码，不在主线程调用
pub fn open(app: &AppHandle, src: Source) -> Result<(), AppError> {
    open_noted(app, src, None)
}

/// 同 `open`，窗口里另给一句提示
fn open_noted(app: &AppHandle, src: Source, note: Option<&'static str>) -> Result<(), AppError> {
    let from = src.kind();
    let entry = match src {
        Source::Panel { .. } => Entry::Panel,
        Source::Blank => Entry::Tray,
        _ => Entry::Other,
    };
    open_as(app, src, from, Some(entry), note)
}

/// 主线程上的入口用：到后台打开，打不开的提示
pub fn open_bg(app: &AppHandle, src: Source) {
    open_noted_bg(app, src, None);
}

fn open_noted_bg(app: &AppHandle, src: Source, note: Option<&'static str>) {
    let app = app.clone();
    crate::housekeeping::spawn("xpaste-textedit-open", move || {
        if let Err(e) = open_noted(&app, src, note) {
            toast(&app, &e.to_string());
        }
    });
}

/// 窗口里换一张（`⌘V`、最近的图、最近改过）：来处和粘贴目标不变
pub fn open_inside(app: &AppHandle, src: Source, from: &'static str) -> Result<(), AppError> {
    open_as(app, src, from, None, None)
}

/// 拖进窗口的文件：同 `open_inside`，多个的只用一个
pub fn open_dropped(app: &AppHandle, paths: &[PathBuf]) -> Result<(), AppError> {
    let Some((p, note)) = pick_file(paths) else {
        return Ok(());
    };
    open_as(app, Source::File(p.into()), "file", None, note)
}

// ---- 系统的“打开方式” ----

/// 改得了的图片文件的扩展名
const EXTS: [&str; 8] = ["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif"];

fn is_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// 命令行参数里的图片文件；第一项是程序自己
pub fn argv_files(argv: &[String]) -> Vec<PathBuf> {
    argv.iter()
        .skip(1)
        .filter(|a| !a.starts_with('-') && !a.starts_with("xpaste://"))
        .map(PathBuf::from)
        .filter(|p| is_image(p))
        .collect()
}

/// 用哪个文件：第一个图片，一个图片都没有就第一个；给了多个的带一句提示
fn pick_file(files: &[PathBuf]) -> Option<(&Path, Option<&'static str>)> {
    let p = files.iter().find(|p| is_image(p)).or(files.first())?;
    Some((p, (files.len() > 1).then_some(ONE)))
}

type Picked<'a> = (&'a Path, Option<&'static str>);

/// 打开哪个文件；没开启时是给用户看的原因；没有文件为 None
fn route_files(on: bool, files: &[PathBuf]) -> Option<Result<Picked<'_>, &'static str>> {
    pick_file(files).map(|p| on.then_some(p).ok_or(OFF))
}

/// 应用还没启动完就来的文件（macOS 上被文件拉起时系统在启动完成之前就发来）
static EARLY: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// 系统“打开方式”带来的文件。没有文件、这个构建没有改字时返回 false，调用方照常
pub fn opened(app: &AppHandle, files: Vec<PathBuf>) -> bool {
    if files.is_empty() || !crate::hotkey::edit_enabled() {
        return false;
    }
    if app.try_state::<AppState>().is_none() {
        *lock(&EARLY) = files;
        return true;
    }
    match route_files(on(app), &files) {
        None => return false,
        Some(Ok((p, note))) => open_noted_bg(app, Source::File(p.into()), note),
        Some(Err(why)) => {
            log::info!("改字 没有开启，不打开文件");
            // 面板页面还没就绪（刚启动）时它收不到，改发系统通知
            if crate::panel::page_ready() {
                crate::panel::show(app, None, crate::panel::Trigger::Tray);
                // 排在面板显示之后发：panel-shown 会重画列表
                on_main(app, |app| emit_to(app, "panel", events::TE_OFF, ()));
            } else {
                crate::notify(app, why);
            }
        }
    }
    true
}

/// 启动完成：打开启动参数和启动完成之前来的文件
pub fn start(app: &AppHandle) {
    let argv: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let mut files = std::mem::take(&mut *lock(&EARLY));
    files.extend(argv_files(&argv));
    opened(app, files);
}

fn open_as(
    app: &AppHandle,
    mut src: Source,
    from: &'static str,
    entry: Option<Entry>,
    note: Option<&'static str>,
) -> Result<(), AppError> {
    check_on(app)?;
    let seq = OPENS.fetch_add(1, Ordering::SeqCst) + 1;
    let origin = entry.map(|e| meme::arrive(None, e, meme::foreground));
    let expired = expired(app);
    let live = match &src {
        Source::Blank => None,
        Source::Recent(id) => Some(find(app, id)?),
        // 试用结束只能看“最近改过”的
        _ if expired => None,
        _ => {
            let bytes = bytes_of(&app.state::<AppState>(), &mut src)?;
            let (img, icc) = decode(&bytes).map_err(|e| no(e.text()))?;
            Some(enlist(Live::fresh(img, icc), None))
        }
    };
    let (target, fresh) = {
        let mut rt = lock(&RT);
        // 又开了一张，或者窗口里换图时窗口已经关了
        if (origin.is_none() && !rt.open) || !admit(&mut rt.shown, seq) {
            return Ok(());
        }
        if let Some(o) = origin {
            rt.origin = o;
        }
        let fresh = !std::mem::replace(&mut rt.open, true);
        rt.unseen = 0;
        rt.cur.clone_from(&live);
        (rt.origin.target.clone(), fresh)
    };
    tasks().sweep();
    log::info!(
        "改字 打开 {from}{}{}{}",
        live.as_ref()
            .map_or(String::new(), |l| format!(" {}×{}", l.img.w, l.img.h)),
        if target.is_some() {
            "（有粘贴目标）"
        } else {
            ""
        },
        if expired { "（试用已结束）" } else { "" },
    );
    if crate::panel::visible(app) {
        crate::panel::hide(app);
    }
    let payload = OpenPayload {
        job: live.map(|l| l.view()),
        from,
        target,
        expired,
        config: config(app),
    };
    show(app, (payload, note), fresh);
    Ok(())
}

/// 显示窗口，把载荷发给页面；页面还没把事件听上时先留着。`fresh` 时先摆好位置
fn show(app: &AppHandle, opening: Opening, fresh: bool) {
    on_main(app, move |app| {
        textedit_window::show(app, fresh);
        let now = lock(&RT).deliver(opening);
        send(app, now);
    });
}

/// 页面说事件都听上了：补发留着的那次打开
pub fn ready(app: &AppHandle) {
    on_main(app, |app| {
        let held = lock(&RT).page_ready();
        send(app, held);
    });
}

fn send(app: &AppHandle, opening: Option<Opening>) {
    let Some((payload, note)) = opening else {
        return;
    };
    emit_to(app, LABEL, events::TE_OPEN, payload);
    if let Some(note) = note {
        emit_to(app, LABEL, events::TE_TOAST, note);
    }
}

/// 提示：窗口开着在窗口里，否则发系统通知
fn toast(app: &AppHandle, text: &str) {
    if window_open() {
        emit_to(app, LABEL, events::TE_TOAST, text);
    } else {
        crate::notify(app, text);
    }
}

pub fn window_open() -> bool {
    lock(&RT).open
}

/// 窗口记成关闭，放开那张图（有任务的由任务拿着），返回来处；没开着时返回 None
fn shut() -> Option<Origin> {
    let (origin, _cur) = {
        let mut rt = lock(&RT);
        // 还没解完的那几次打开作废
        rt.shown = OPENS.load(Ordering::SeqCst);
        if !rt.open {
            return None;
        }
        rt.open = false;
        rt.held = None;
        (
            std::mem::replace(&mut rt.origin, meme::NOWHERE),
            rt.cur.take(),
        )
    };
    tasks().sweep();
    Some(origin)
}

/// 关掉窗口，键盘回到原来的应用。没开着时不做事
pub fn close(app: &AppHandle) {
    if let Some(o) = shut() {
        log::info!("改字 关闭窗口");
        textedit_window::hide(app, o.back);
    }
}

/// 返回面板：粘贴目标还是打开改字前的那个
pub fn back(app: &AppHandle) {
    let Some(o) = shut() else { return };
    log::info!("改字 返回面板");
    textedit_window::hide(app, None);
    app.state::<AppState>()
        .previous_window
        .store(o.back.unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
    crate::panel::reshow(app);
}

/// 窗口重新拿到焦点：更新并返回粘贴目标的名字（macOS 换成此刻最前面的应用，Windows 不变）
pub fn refresh_target() -> Option<String> {
    #[cfg(target_os = "macos")]
    let front = meme::front_name();
    #[cfg(not(target_os = "macos"))]
    let front = None;
    let mut rt = lock(&RT);
    if !rt.open {
        return None;
    }
    meme::retarget(&mut rt.origin, cfg!(target_os = "macos"), || front)
}

/// 窗口关着时又改好一处：到现在共几处；窗口开着为 None
pub fn missed() -> Option<u32> {
    let mut rt = lock(&RT);
    if rt.open {
        return None;
    }
    rt.unseen += 1;
    Some(rt.unseen)
}

// ---- 最近改过 ----

/// “最近改过”的一张；`done` 是画上去的有几处
#[derive(Serialize)]
pub struct Recent {
    id: String,
    w: u32,
    h: u32,
    created: i64,
    done: usize,
}

const RECENT_MAX: usize = 12;

pub fn recent(dir: &Path) -> Vec<Recent> {
    store::list(dir)
        .into_iter()
        .take(RECENT_MAX)
        .map(|j| Recent {
            done: j.edits.iter().filter(|e| e.shown().is_some()).count(),
            id: j.id,
            w: j.w,
            h: j.h,
            created: j.created,
        })
        .collect()
}

/// 缩略图的 PNG，长边不超过 320。一张一张来：都是整张解出来再缩，十几张一起会占很多内存
pub fn thumb(dir: &Path, id: &str) -> Result<Vec<u8>, AppError> {
    static ONE: Mutex<()> = Mutex::new(());
    let _one = lock(&ONE);
    let img = store::load(dir, id)
        .and_then(|j| store::render(dir, &j))
        .map_err(|_| no(GONE))?;
    let img = image::RgbaImage::from_raw(img.w, img.h, img.px).ok_or_else(|| no(GONE))?;
    let img = DynamicImage::ImageRgba8(img).thumbnail(320, 320);
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|_| no(GONE))?;
    Ok(png)
}

/// 删一张：排队的任务拿掉
pub fn remove(t: Tasks, dir: &Path, id: &str) -> Result<(), AppError> {
    store::remove(dir, id).map_err(not_saved)?;
    t.cancel_gone(dir);
    Ok(())
}

/// 撤销删除，没存进去的结果再存一次
pub fn restore(t: Tasks, dir: &Path, id: &str) -> Result<(), AppError> {
    store::restore(dir, id).map_err(not_saved)?;
    job::retry_late(t, dir, id);
    Ok(())
}

/// 清空：正拿着的图下次再存时重新落盘
pub fn clear(t: Tasks, dir: &Path) -> Result<(), AppError> {
    let out = store::clear(dir);
    t.cancel_gone(dir);
    let all: Vec<Arc<Live>> = lock(&LIVE).iter().filter_map(Weak::upgrade).collect();
    for l in all {
        l.wiped(t, dir);
    }
    Ok(out?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::textedit_job::tests::*;
    use crate::textedit_tasks::tests::tasks as new_tasks;

    const RED: [u8; 3] = [255, 0, 0];
    const BLUE: [u8; 3] = [0, 0, 255];

    /// 方向 6（拍的时候转了 90°）：摆正后宽高对调，左上角到右上角
    #[test]
    fn decode_exif_rotates() {
        let img = image::RgbImage::from_fn(32, 16, |x, y| {
            image::Rgb(if x < 8 && y < 8 { RED } else { [255; 3] })
        });
        let mut jpg = Vec::new();
        img.write_to(&mut Cursor::new(&mut jpg), image::ImageFormat::Jpeg)
            .unwrap();
        let mut exif =
            b"Exif\0\0MM\0\x2a\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01\0\x06\0\0\0\0\0\0".to_vec();
        let mut seg = vec![0xFF, 0xE1];
        seg.extend((exif.len() as u16 + 2).to_be_bytes());
        seg.append(&mut exif);
        jpg.splice(2..2, seg);
        let (img, icc) = decode(&jpg).unwrap();
        assert_eq!((img.w, img.h), (16, 32));
        assert!(icc.is_none());
        let px = |x: u32, y: u32| &img.px[((y * img.w + x) * 4) as usize..][..3];
        assert!(px(13, 2)[1] < 90 && px(2, 2)[1] > 200 && px(13, 28)[1] > 200);
    }

    fn chunk(kind: &[u8], body: &[u8]) -> Vec<u8> {
        let mut c = (body.len() as u32).to_be_bytes().to_vec();
        let data = [kind, body].concat();
        let crc = !data.iter().fold(!0u32, |c, &b| {
            (0..8).fold(c ^ u32::from(b), |c, _| {
                (c >> 1) ^ (0xEDB8_8320 & (c & 1).wrapping_neg())
            })
        });
        c.extend(data);
        c.extend(crc.to_be_bytes());
        c
    }

    /// 只有头的 PNG
    fn png_head(w: u32, h: u32) -> Vec<u8> {
        let ihdr = [&w.to_be_bytes()[..], &h.to_be_bytes(), &[8, 6, 0, 0, 0]].concat();
        [
            b"\x89PNG\r\n\x1a\n".to_vec(),
            chunk(b"IHDR", &ihdr),
            chunk(b"IDAT", &[0x78, 0x9c, 3, 0, 0, 0, 0, 1]),
            chunk(b"IEND", &[]),
        ]
        .concat()
    }

    /// 先看头部的尺寸：没有像素数据也能判断，说明没有解
    #[test]
    fn decode_too_big() {
        for (w, h) in [(20_000, 20_000), (10_001, 10_000)] {
            assert_eq!(decode(&png_head(w, h)).err(), Some(OpenFail::TooBig));
        }
        // 正好 1 亿像素的不算大，只是这个文件没有数据
        let r = decode(&png_head(10_000, 10_000));
        assert_eq!(r.err(), Some(OpenFail::Unreadable));
    }

    #[test]
    fn decode_gif_first() {
        use image::codecs::gif::GifEncoder;
        let mut gif = Vec::new();
        let mut enc = GifEncoder::new(&mut gif);
        for c in [[255, 0, 0, 255], [0, 0, 255, 255]] {
            let f = image::RgbaImage::from_pixel(3, 2, image::Rgba(c));
            enc.encode_frame(image::Frame::new(f)).unwrap();
        }
        drop(enc);
        let (img, _) = decode(&gif).unwrap();
        assert_eq!((img.w, img.h), (3, 2));
        assert_eq!(img.px[..4], [255, 0, 0, 255]);
    }

    #[test]
    fn decode_garbage() {
        for b in [&b""[..], b"hello", b"\x89PNG\r\n\x1a\n", b"GIF89a"] {
            assert_eq!(decode(b).err(), Some(OpenFail::Unreadable));
        }
    }

    /// RGB 的 ICC 留着，别的不要
    #[test]
    fn decode_keeps_rgb_icc() {
        for (space, kept) in [(b"RGB ", true), (b"GRAY", false), (b"CMYK", false)] {
            let mut icc = vec![7u8; 200];
            icc[16..20].copy_from_slice(space);
            let png = crate::img_meta::encode_png(2, 1, &[9; 8], Some(&icc)).unwrap();
            let (img, got) = decode(&png).unwrap();
            assert_eq!(img.px, [9; 8]);
            assert_eq!(got, kept.then_some(icc));
        }
    }

    #[test]
    fn source_kinds() {
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        let all = [
            Source::Shot { rect, png: vec![] },
            Source::Clip { record: 1 },
            Source::Panel {
                record: 1,
                file: None,
            },
            Source::Pin { n: 1 },
            Source::File("a.png".into()),
            Source::Blank,
            Source::Recent("x".into()),
        ];
        assert_eq!(
            all.map(|s| s.kind()),
            ["shot", "clip", "panel", "pin", "file", "blank", "recent"]
        );
    }

    /// 记录的图：文件记录取第一个图片文件，没有图的、没了的各有说法
    #[test]
    fn bytes_of_sources() {
        use xpaste_core::{Snapshot, capture, detect, store::Upsert};
        let tmp = Tmp::new();
        let state = AppState::for_test();
        for (f, b) in [("a.pdf", "x"), ("b.png", "png"), ("c.png", "c")] {
            std::fs::write(tmp.0.join(f), b).unwrap();
        }
        let snap = Snapshot {
            file_paths: ["a.pdf", "b.png", "c.png"].map(|f| tmp.0.join(f)).to_vec(),
            ..Default::default()
        };
        let kind = detect::kind(&snap);
        let rec = capture::build_record(snap, kind, 0);
        let (Upsert::Added(record) | Upsert::Merged(record)) =
            state.store().insert_or_merge(&rec, 0, 500).unwrap().0;
        let get = |mut src| bytes_of(&state, &mut src);
        let panel = |record, file| get(Source::Panel { record, file });
        assert_eq!(panel(record, None).unwrap(), b"png");
        assert_eq!(get(Source::Clip { record }).unwrap(), b"png");
        assert_eq!(panel(record, Some(2)).unwrap(), b"c");
        assert_eq!(get(Source::File(tmp.0.join("c.png"))).unwrap(), b"c");
        assert_eq!(msg(panel(999, None)), UNUSABLE);
        // 文件夹、设备不收
        #[cfg(unix)]
        assert_eq!(msg(get(Source::File("/dev/null".into()))), "这张图读不出来");
        std::fs::create_dir(tmp.0.join("夹子.png")).unwrap();
        assert_eq!(
            msg(get(Source::File(tmp.0.join("夹子.png")))),
            "这张图读不出来"
        );
        assert_eq!(msg(get(Source::Pin { n: 1 << 40 })), "这张图读不出来");
        std::fs::remove_file(tmp.0.join("b.png")).unwrap();
        assert_eq!(msg(get(Source::Clip { record })), "这张图读不出来");
    }

    #[test]
    fn closed_or_expired_not_editable() {
        assert_eq!(may_edit(false, || panic!("没开启不查试用期")), Err(OFF));
        assert_eq!(may_edit(true, || true), Err(EXPIRED));
        assert_eq!(may_edit(true, || false), Ok(()));
    }

    /// 没改过的不落盘；打开着的、从盘上读回来的是同一份
    #[test]
    fn stored_on_first_edit() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (new_tasks(), Tmp::new());
        let l = live();
        l.plan(&draft(NEW, range(4, 5)), false, true).unwrap();
        assert_eq!(std::fs::read_dir(&tmp.0).unwrap().count(), 0);
        assert!(l.render(&tmp.0).is_ok());
        let id = put_local(t, &tmp.0, &l, &draft(NEW, range(4, 5)), 4, RED).unwrap();
        let back = Live::load(&tmp.0, &l.id).unwrap();
        assert!(back.img.px == l.img.px);
        assert!(back.view().rev > l.view().rev, "重新打开不归零");
        assert_eq!(edit_of(&back, id).pick, 0);
        let r = recent(&tmp.0);
        assert_eq!((r.len(), r[0].id == l.id, r[0].done), (1, true, 1));
        let th = image::load_from_memory(&thumb(&tmp.0, &l.id).unwrap()).unwrap();
        assert_eq!((th.width(), th.height()), (320, 100));
        let file = &edit_of(&l, id).cands[0].file;
        assert!(l.cand_png(&tmp.0, file).unwrap().starts_with(b"\x89PNG"));
        assert!(l.cand_png(&tmp.0, "orig.png").is_err());
        // 清空之后接着改：重新落盘
        clear(t, &tmp.0).unwrap();
        l.wiped(t, &tmp.0);
        assert!(recent(&tmp.0).is_empty() && edit_of(&l, id).cands.is_empty());
        put_local(t, &tmp.0, &l, &draft(NEW, range(4, 5)), 4, BLUE).unwrap();
        assert_eq!(recent(&tmp.0).len(), 1);
        assert_eq!(at(&l, &tmp.0, 76 + 260, 90), BLUE);
    }

    /// 先开的解得慢：后开的上了屏就不换回去；后开的打不开，先开的照样上屏
    #[test]
    fn slow_open_does_not_come_back() {
        let mut shown = 0;
        assert!(admit(&mut shown, 2) && !admit(&mut shown, 1));
        let mut shown = 0;
        // 2 号打不开，没来
        assert!(admit(&mut shown, 1) && admit(&mut shown, 3));
        // 关了窗口：还没解完的作废，之后的照常
        let mut shown = 5;
        assert!(!admit(&mut shown, 4) && !admit(&mut shown, 5) && admit(&mut shown, 6));
    }

    /// 读进来之后、登记之前，别处改过又放手了：以盘上的为准
    #[test]
    fn enlist_rereads_job() {
        let tmp = Tmp::new();
        let l = live();
        let first = job_on_disk(&l, &tmp.0);
        let stale = Live::load(&tmp.0, &l.id).unwrap();
        stale.no_reading();
        let mut job = first;
        job.edits.push(store::Edit {
            id: 7,
            line: LINE,
            range: [0.0, 1.0],
            old: "甲".into(),
            new: "乙".into(),
            pick: store::NO_PICK,
            cands: vec![],
        });
        store::save(&tmp.0, &job).unwrap();
        let got = enlist(stale, Some(&tmp.0));
        assert_eq!(got.view().edits.len(), 1);
        assert_eq!(got.view().edits[0].id, 7);
    }

    #[test]
    fn argv_files_picks_images_only() {
        let argv = [
            r"C:\app\图.png",
            "--flag.png",
            "-x.jpg",
            "xpaste://activate?code=a.png",
            "a.txt",
            "b.PNG",
            "/tmp/c d.jpeg",
            "e.heic",
            "f.webp",
            "g.bmp",
            "h.tif",
            "i.tiff",
            "j.gif",
            "k.jpg",
            "png",
            "",
        ]
        .map(String::from);
        let want = [
            "b.PNG",
            "/tmp/c d.jpeg",
            "f.webp",
            "g.bmp",
            "h.tif",
            "i.tiff",
            "j.gif",
            "k.jpg",
        ];
        assert_eq!(argv_files(&argv), want.map(PathBuf::from));
        assert!(argv_files(&[]).is_empty());
    }

    #[test]
    fn argv_two_files_takes_first() {
        let argv = ["xpaste", "a.png", "b.png"].map(String::from);
        let files = argv_files(&argv);
        assert_eq!(
            pick_file(&files),
            Some((Path::new("a.png"), Some("一次改一张")))
        );
        assert_eq!(pick_file(&files[..1]), Some((Path::new("a.png"), None)));
        assert_eq!(pick_file(&[]), None);
        // 拖进来的夹着别的：取第一个图片；一个都不是就给第一个，照常说读不出来
        let mixed = ["a.txt", "b.png"].map(PathBuf::from);
        assert_eq!(
            pick_file(&mixed),
            Some((Path::new("b.png"), Some("一次改一张")))
        );
        assert_eq!(pick_file(&mixed[..1]), Some((Path::new("a.txt"), None)));
    }

    #[test]
    fn files_while_disabled_toasts() {
        let files = ["a.png", "b.png"].map(PathBuf::from);
        assert_eq!(route_files(false, &files), Some(Err("改字没有开启")));
        assert_eq!(
            route_files(true, &files),
            Some(Ok((Path::new("a.png"), Some("一次改一张"))))
        );
        assert_eq!(route_files(true, &[]), None);
        assert_eq!(route_files(false, &[]), None);
    }

    /// 页面没就绪：先留着最后一次打开（连同提示），就绪后补发一次；就绪以后直接发
    #[test]
    fn open_before_window_ready_is_held() {
        let p = |from| OpenPayload {
            job: None,
            from,
            target: None,
            expired: false,
            config: Config {
                mode: "local".into(),
                api: false,
            },
        };
        let mut rt = Runtime::new();
        assert!(rt.deliver((p("blank"), None)).is_none());
        assert!(rt.deliver((p("file"), Some(ONE))).is_none());
        let (held, note) = rt.page_ready().unwrap();
        assert_eq!((held.from, note), ("file", Some("一次改一张")));
        assert!(rt.page_ready().is_none());
        assert_eq!(rt.deliver((p("shot"), None)).unwrap().0.from, "shot");
        // 页面重新加载，没就绪之前关掉的不补发
        let mut rt = Runtime::new();
        assert!(rt.deliver((p("blank"), None)).is_none());
        rt.held = None;
        assert!(rt.page_ready().is_none());
    }

    fn job_on_disk(l: &Live, dir: &Path) -> store::Job {
        let mut job = store::Job::new(&l.img);
        job.id.clone_from(&l.id);
        store::create(dir, &job, &l.img, None).unwrap();
        job
    }

    /// 有人拿着的图只有一份
    #[test]
    fn one_live_per_job() {
        let l = enlist(Live::fresh(scene(), None), None);
        let mut twin = Live::fresh(scene(), None);
        twin.id.clone_from(&l.id);
        assert!(Arc::ptr_eq(&l, &enlist(twin, None)));
        let id = l.id.clone();
        // 读字的线程放手之后
        l.read();
        drop(l);
        for _ in 0..100 {
            if !lock(&LIVE)
                .iter()
                .any(|w| w.upgrade().is_some_and(|l| l.id == id))
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("没有放开");
    }
}
