//! 改字：打开一张图，和改字窗口的运行时（只在带 Pro 的构建里启用）。一张图里的事见 `textedit_job`，AI 任务见 `textedit_tasks`。
//!
//! 窗口只拿着当前那张图，没结束的任务各自拿着自己那张：换图、关窗口都不影响它们。

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use image::{DynamicImage, ImageDecoder, ImageReader};
use serde::Serialize;
use tauri::{AppHandle, Manager};
use xpaste_pro::TeImg;

use crate::error::AppError;
use crate::events::{self, emit_to};
use crate::meme::{self, Entry, Front, Origin};
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
#[derive(Clone)]
pub enum Source {
    Shot { rect: Rect, png: Vec<u8> },
    Clip { record: i64 },
    Panel { record: i64, file: Option<u32> },
    Pin { n: u64 },
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

/// 摆正后的宽高，只读文件头
fn head(r: impl std::io::BufRead + std::io::Seek) -> Option<(u32, u32)> {
    let r = ImageReader::new(r).with_guessed_format().ok()?;
    let mut d = r.into_decoder().ok()?;
    let (w, h) = d.dimensions();
    use image::metadata::Orientation::*;
    Some(match d.orientation().unwrap_or(NoTransforms) {
        Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH => (h, w),
        _ => (w, h),
    })
}

/// 图片文件摆正后的宽高
pub fn dims(path: &Path) -> Option<(u32, u32)> {
    head(std::io::BufReader::new(std::fs::File::open(path).ok()?))
}

/// 来源的图片文件；`Shot` 没有
fn path_of(state: &AppState, src: &Source) -> Result<PathBuf, AppError> {
    let unreadable = || no(OpenFail::Unreadable.text());
    let path = match src {
        Source::Clip { record } => record_path(state, *record, None)?,
        Source::Panel { record, file } => record_path(state, *record, *file)?,
        Source::Pin { n } => state.data_dir.join("pins").join(format!("{n}.png")),
        Source::File(p) => p.clone(),
        Source::Shot { .. } | Source::Blank | Source::Recent(_) => return Err(unreadable()),
    };
    // 只收普通文件：管道、设备读起来没有头
    let m = std::fs::metadata(&path).map_err(|_| unreadable())?;
    if !m.is_file() {
        return Err(unreadable());
    }
    if m.len() > MAX_BYTES {
        return Err(no(OpenFail::TooBig.text()));
    }
    Ok(path)
}

/// 来源的图片文件字节
fn bytes_of(state: &AppState, src: &mut Source) -> Result<Vec<u8>, AppError> {
    if let Source::Shot { png, .. } = src {
        return Ok(std::mem::take(png));
    }
    let path = path_of(state, src)?;
    std::fs::read(&path).map_err(|_| no(OpenFail::Unreadable.text()))
}

/// 不解码，只按文件头看打不打得开
fn peek(state: &AppState, src: &Source) -> Result<(), AppError> {
    let size = match src {
        Source::Shot { png, .. } => head(Cursor::new(png)),
        _ => dims(&path_of(state, src)?),
    };
    let (w, h) = size.ok_or_else(|| no(OpenFail::Unreadable.text()))?;
    if u64::from(w) * u64::from(h) > MAX_PX {
        return Err(no(OpenFail::TooBig.text()));
    }
    Ok(())
}

/// 打开之前先看行不行：开着、图打得开（试用结束时不看图，照常开窗口）
pub fn check(app: &AppHandle, src: &Source) -> Result<(), AppError> {
    check_on(app)?;
    if expired(app) {
        return Ok(());
    }
    peek(&app.state::<AppState>(), src)
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
    /// `cur` 的来源，关了窗口就忘掉：原文件的路径、截图的位置、贴图只在开着时记
    src: Source,
    /// 窗口关着时改好了几处
    unseen: u32,
    /// 上屏的（或作废的）最大的打开号
    shown: u64,
    /// 正在打开“打开方式”来的文件；同一批后到的让它打开后提示一句
    batch: Option<Option<&'static str>>,
}

/// 窗口里此刻的一张图
pub struct Current {
    pub live: Arc<Live>,
    pub src: Source,
    pub target: Option<String>,
    /// 这一次打开的号
    pub seq: u64,
}

/// 完成时按什么来源算：窗口里 `⌘V`、最近的图是空白窗口来的
fn kept(src: Source, from: &str) -> Source {
    if from == "blank" { Source::Blank } else { src }
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
            src: Source::Blank,
            unseen: 0,
            shown: 0,
            batch: None,
        }
    }

    /// 换上 `seq` 这一次打开的，返回粘贴目标。来处跟着换；窗口里换图的没有来处（不变）。
    /// 又开了一张、窗口里换图时窗口已经关了的不换，返回 None
    fn enter(
        &mut self,
        seq: u64,
        origin: Option<Origin>,
        live: Option<Arc<Live>>,
        src: Source,
    ) -> Option<Option<String>> {
        if (origin.is_none() && !self.open) || !admit(&mut self.shown, seq) {
            return None;
        }
        if let Some(o) = origin {
            self.origin = o;
        }
        self.open = true;
        self.unseen = 0;
        self.src = live.as_ref().map_or(Source::Blank, |_| src);
        self.cur = live;
        Some(self.origin.target.clone())
    }

    /// 窗口里开着的那张；`id` 不是它的没有
    fn current(&self, id: &str) -> Option<Current> {
        let live = self.cur.as_ref().filter(|l| self.open && l.id == id)?;
        Some(Current {
            live: live.clone(),
            src: self.src.clone(),
            target: self.origin.target.clone(),
            seq: self.shown,
        })
    }

    /// 窗口里的还是不是这一次打开的（后面又开了一张、已经关了就不是）
    fn shows(&self, seq: u64) -> bool {
        self.open && self.shown == seq
    }

    /// 开着一张图：再点“改字…”时留着它
    fn busy(&self) -> bool {
        self.open && self.cur.is_some()
    }

    /// 页面就绪了就交回去发，没有就先留着
    fn deliver(&mut self, mut o: Opening) -> Option<Opening> {
        if o.0.from == "file" {
            o.1 = o.1.or(self.batch.take().flatten());
        }
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

    /// 同一批又来了文件：提示跟着那次打开一起发。返回要不要现在就发（已经打开了）
    fn extra(&mut self) -> bool {
        if let Some(note) = &mut self.batch {
            *note = Some(ONE);
        } else if let Some(held) = &mut self.held {
            held.1 = Some(ONE);
        } else {
            return self.open;
        }
        false
    }

    /// 记成关闭，`opens` 号为止还没上屏的打开作废；没开着时返回 None
    fn shut(&mut self, opens: u64) -> Option<(Origin, Option<Arc<Live>>)> {
        self.shown = opens;
        if !self.open {
            return None;
        }
        self.open = false;
        self.held = None;
        self.src = Source::Blank;
        let origin = std::mem::replace(&mut self.origin, meme::NOWHERE);
        Some((origin, self.cur.take()))
    }

    /// 同 `shut`，窗口里已经不是 `seq` 这一次打开的就不关
    fn shut_if(&mut self, seq: u64, opens: u64) -> Option<(Origin, Option<Arc<Live>>)> {
        self.shows(seq).then(|| self.shut(opens)).flatten()
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
    open_from(app, src, None, None)
}

/// 同 `open`，窗口里另给一句提示 `note`；`front` 是已经记好的前台
fn open_from(
    app: &AppHandle,
    src: Source,
    note: Option<&'static str>,
    front: Option<Front>,
) -> Result<(), AppError> {
    let from = src.kind();
    let entry = match src {
        Source::Panel { .. } => Entry::Panel,
        Source::Blank => Entry::Tray,
        _ => Entry::Other,
    };
    open_as(app, src, from, Some((entry, front)), note)
}

/// 主线程上的入口用：到后台打开，打不开的提示
pub fn open_bg(app: &AppHandle, src: Source) {
    spawn_open(app, src, None, None);
}

/// 同 `open_bg`，前台已经记好（从截图遮罩进来）
pub fn open_bg_from(app: &AppHandle, src: Source, front: Front) {
    spawn_open(app, src, None, Some(front));
}

fn spawn_open(app: &AppHandle, src: Source, note: Option<&'static str>, front: Option<Front>) {
    let app = app.clone();
    let file = matches!(src, Source::File(_));
    crate::housekeeping::spawn("xpaste-textedit-open", move || {
        if let Err(e) = open_from(&app, src, note, front) {
            if file {
                lock(&RT).batch = None;
            }
            toast(&app, &e.to_string());
        }
    });
}

/// 来处：从窗口里换图的没有（不变）；前台没有记好的按这一刻取
fn origin_of(entry: Option<(Entry, Option<Front>)>, now: impl FnOnce() -> Front) -> Option<Origin> {
    entry.map(|(e, front)| meme::arrive(None, e, || front.unwrap_or_else(now)))
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

/// 接连到的文件算同一批：Windows 上选中几个一起打开，会起几个进程、一个一个转发过来
struct Batch {
    last: Option<Instant>,
    /// 这一批提示过“一次改一张”了
    noted: bool,
}

#[derive(Debug, PartialEq)]
enum Join {
    Open,
    /// 同一批后到的：不打开，提示一次
    Note,
    Skip,
}

const BATCH_GAP: Duration = Duration::from_secs(1);

impl Batch {
    /// 这一刻又来了 `n` 个文件
    fn join(&mut self, now: Instant, n: usize) -> Join {
        let same = self
            .last
            .replace(now)
            .is_some_and(|t| now.saturating_duration_since(t) <= BATCH_GAP);
        if !same {
            self.noted = n > 1;
            Join::Open
        } else if std::mem::replace(&mut self.noted, true) {
            Join::Skip
        } else {
            Join::Note
        }
    }
}

static BATCH: Mutex<Batch> = Mutex::new(Batch {
    last: None,
    noted: false,
});

/// 遮罩或长截图结束后再打开的文件
static WAITING: Mutex<Option<Vec<PathBuf>>> = Mutex::new(None);

/// 面板还没就绪时来的“改字没有开启”，就绪后补发
static OFF_DUE: AtomicBool = AtomicBool::new(false);

/// 系统“打开方式”带来的文件。没有文件、这个构建没有改字时返回 false，调用方照常
pub fn opened(app: &AppHandle, files: Vec<PathBuf>) -> bool {
    if files.is_empty() || !crate::hotkey::edit_enabled() {
        return false;
    }
    if app.try_state::<AppState>().is_none() {
        lock(&EARLY).extend(files);
        return true;
    }
    let join = lock(&BATCH).join(Instant::now(), files.len());
    match join {
        Join::Open if crate::panel::under_overlay(app) => wait(app, files),
        Join::Open => open_files(app, &files),
        Join::Note => {
            if let Some(w) = &mut *lock(&WAITING) {
                w.extend(files);
            } else if on(app) && lock(&RT).extra() {
                emit_to(app, LABEL, events::TE_TOAST, ONE);
            }
        }
        Join::Skip => {}
    }
    true
}

/// 等遮罩、长截图结束再打开（窗口会压在遮罩下面、拿走键盘）；等的时候又来一批，用后来的
fn wait(app: &AppHandle, files: Vec<PathBuf>) {
    if lock(&WAITING).replace(files).is_some() {
        return;
    }
    let app = app.clone();
    // ponytail: 每 300 ms 看一次，不在遮罩、长截图的每条结束路径上挂钩子
    let spawned = crate::housekeeping::spawn("xpaste-textedit-wait", move || {
        while crate::panel::under_overlay(&app) {
            std::thread::sleep(Duration::from_millis(300));
        }
        let files = lock(&WAITING).take();
        if let Some(files) = files {
            open_files(&app, &files);
        }
    });
    if !spawned {
        *lock(&WAITING) = None;
    }
}

fn open_files(app: &AppHandle, files: &[PathBuf]) {
    match route_files(on(app), files) {
        None => {}
        Some(Ok((p, note))) => {
            lock(&RT).batch = Some(None);
            spawn_open(app, Source::File(p.into()), note, None);
        }
        Some(Err(_)) => {
            log::info!("改字 没有开启，不打开文件");
            crate::panel::show(app, None, crate::panel::Trigger::Tray);
            OFF_DUE.store(true, Ordering::SeqCst);
            if crate::panel::page_ready() {
                panel_ready(app);
            }
        }
    }
}

/// 面板页面就绪（刚启动）：补发“改字没有开启”。排在面板显示之后发：panel-shown 会重画列表
pub fn panel_ready(app: &AppHandle) {
    if OFF_DUE.swap(false, Ordering::SeqCst) {
        on_main(app, |app| emit_to(app, "panel", events::TE_OFF, ()));
    }
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
    entry: Option<(Entry, Option<Front>)>,
    note: Option<&'static str>,
) -> Result<(), AppError> {
    check_on(app)?;
    // 开着一张图时再点“改字…”：只拿到前面，不换成空白的
    if matches!(src, Source::Blank) && lock(&RT).busy() {
        log::info!("改字 窗口拿到前面");
        if crate::panel::visible(app) {
            crate::panel::hide(app);
        }
        on_main(app, |app| {
            if window_open() {
                textedit_window::show(app);
            }
        });
        return Ok(());
    }
    let seq = OPENS.fetch_add(1, Ordering::SeqCst) + 1;
    let origin = origin_of(entry, meme::foreground);
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
    let entered = lock(&RT).enter(seq, origin, live.clone(), kept(src, from));
    let Some(target) = entered else {
        return Ok(());
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
    show(app, (payload, note), seq);
    Ok(())
}

/// 显示窗口，把载荷发给页面；页面还没把事件听上时先留着。轮到主线程时已经关了、又开了一张的不显示
fn show(app: &AppHandle, opening: Opening, seq: u64) {
    on_main(app, move |app| {
        if !lock(&RT).shows(seq) {
            return;
        }
        textedit_window::show(app);
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
    let (origin, _cur) = lock(&RT).shut(OPENS.load(Ordering::SeqCst))?;
    tasks().sweep();
    Some(origin)
}

/// 同 `shut`，窗口里已经不是 `seq` 这一次打开的就不关
pub fn shut_if(seq: u64) -> Option<Origin> {
    let (origin, _cur) = lock(&RT).shut_if(seq, OPENS.load(Ordering::SeqCst))?;
    tasks().sweep();
    Some(origin)
}

/// 窗口里开着的那张；`id` 不是它的没有
pub fn current(id: &str) -> Option<Current> {
    lock(&RT).current(id)
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
            align: Default::default(),
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

    fn payload(from: &'static str) -> OpenPayload {
        OpenPayload {
            job: None,
            from,
            target: None,
            expired: false,
            config: Config {
                mode: "local".into(),
                api: false,
            },
        }
    }

    /// 页面没就绪：先留着最后一次打开（连同提示），就绪后补发一次；就绪以后直接发
    #[test]
    fn open_before_window_ready_is_held() {
        let p = payload;
        let mut rt = Runtime::new();
        assert!(rt.deliver((p("blank"), None)).is_none());
        assert!(rt.deliver((p("file"), Some(ONE))).is_none());
        let (held, note) = rt.page_ready().unwrap();
        assert_eq!((held.from, note), ("file", Some("一次改一张")));
        assert!(rt.page_ready().is_none());
        assert_eq!(rt.deliver((p("shot"), None)).unwrap().0.from, "shot");
        // 没就绪之前关掉的不补发，那之前的打开轮到上屏也不显示
        let mut rt = Runtime::new();
        (rt.open, rt.shown) = (true, 3);
        assert!(rt.shows(3) && !rt.shows(2));
        assert!(rt.deliver((p("blank"), None)).is_none());
        assert!(rt.shut(3).is_some());
        assert!(!rt.shows(3));
        assert!(rt.page_ready().is_none());
        assert!(rt.shut(5).is_none());
        assert_eq!(rt.shown, 5);
    }

    /// 选中几个文件一起打开，一个一个到：只开第一个，提示一次
    #[test]
    fn files_in_a_row_are_one_batch() {
        let t = Instant::now();
        let at = |ms| t + Duration::from_millis(ms);
        let new = || Batch {
            last: None,
            noted: false,
        };
        let mut b = new();
        assert_eq!(b.join(at(0), 1), Join::Open);
        assert_eq!(b.join(at(700), 1), Join::Note);
        // 一个接一个，从上一个算起
        assert_eq!(b.join(at(1500), 1), Join::Skip);
        assert_eq!(b.join(at(2600), 1), Join::Open);
        assert_eq!(b.join(at(2700), 1), Join::Note);
        // 一次就给了几个的，打开时已经提示了
        let mut b = new();
        assert_eq!(b.join(at(0), 3), Join::Open);
        assert_eq!(b.join(at(100), 1), Join::Skip);
    }

    /// 后到的提示：还在打开、页面没就绪的跟着那次打开发；开好了现在发；打不开的不发
    #[test]
    fn batch_note_goes_with_its_open() {
        let mut rt = Runtime::new();
        rt.batch = Some(None);
        assert!(!rt.extra());
        rt.ready = true;
        // 别的打开不带走
        assert_eq!(rt.deliver((payload("shot"), None)).unwrap().1, None);
        assert_eq!(rt.deliver((payload("file"), None)).unwrap().1, Some(ONE));
        assert_eq!(rt.deliver((payload("file"), None)).unwrap().1, None);
        let mut rt = Runtime::new();
        rt.open = true;
        assert!(rt.deliver((payload("file"), None)).is_none());
        assert!(!rt.extra());
        assert_eq!(rt.page_ready().unwrap().1, Some(ONE));
        assert!(rt.extra());
        rt.shut(0);
        assert!(!rt.extra());
    }

    /// 开着一张图时“改字…”不换成空白；空白的、没开着的照常
    #[test]
    fn blank_keeps_open_image() {
        let mut rt = Runtime::new();
        assert!(!rt.busy());
        rt.open = true;
        assert!(!rt.busy());
        rt.cur = Some(live());
        assert!(rt.busy());
        rt.shut(0);
        assert!(!rt.busy());
    }

    /// 从遮罩进来用截图前记好的前台，不再问；别处进来问这一刻的
    #[test]
    fn overlay_front_is_kept() {
        let front = || (Some(7), Some("微信".to_string()));
        let o = origin_of(Some((Entry::Other, Some(front()))), || panic!("已经记好")).unwrap();
        assert_eq!((o.back, o.target.as_deref()), (Some(7), Some("微信")));
        // 截图前没有前台窗口也不再问
        let o = origin_of(Some((Entry::Other, Some((None, None)))), || {
            panic!("已经记好")
        });
        assert_eq!(o.unwrap().back, None);
        assert_eq!(
            origin_of(Some((Entry::Other, None)), front).unwrap().back,
            Some(7)
        );
        assert!(origin_of(None, || panic!("窗口里换图不问")).is_none());
        assert_eq!(meme::foreground_of(Some(7)).0, Some(7));
    }

    fn entered(rt: &mut Runtime, seq: u64, entry: Entry, app: &str, src: Source) -> Option<String> {
        let front = (Some(seq as isize), Some(app.to_string()));
        let o = origin_of(Some((entry, None)), || front);
        rt.enter(seq, o, Some(live()), src).expect("上屏了")
    }

    /// 每次从外面打开，粘贴目标按那一刻的前台重新记；窗口里换图不变
    #[test]
    fn target_refreshed_on_each_open() {
        let mut rt = Runtime::new();
        let clip = || Source::Clip { record: 1 };
        // 托盘打开的没有目标；切到微信复制一张图再从快捷键打开
        assert_eq!(
            entered(&mut rt, 1, Entry::Tray, "访达", Source::Blank),
            None
        );
        assert!(rt.origin.tray);
        let t = entered(&mut rt, 2, Entry::Other, "微信", clip());
        assert_eq!(t.as_deref(), Some("微信"));
        assert!(!rt.origin.tray);
        let t = entered(&mut rt, 3, Entry::Panel, "备忘录", clip());
        assert_eq!(t.as_deref(), Some("备忘录"));
        assert_eq!((rt.origin.back, rt.origin.from_panel), (Some(3), true));
        // 窗口里换图
        let t = rt.enter(4, None, Some(live()), Source::Blank).unwrap();
        assert_eq!(t.as_deref(), Some("备忘录"));
        // 晚到的不换
        assert!(rt.enter(3, Some(meme::NOWHERE), None, clip()).is_none());
        assert_eq!(rt.origin.target.as_deref(), Some("备忘录"));
    }

    /// 完成用窗口里此刻这张图的来源；关了就忘掉，只认关着之前的那一次
    #[test]
    fn source_kept_until_closed() {
        let mut rt = Runtime::new();
        let file = Source::File("/a/b.png".into());
        entered(&mut rt, 1, Entry::Other, "访达", file);
        let id = rt.cur.as_ref().unwrap().id.clone();
        let c = rt.current(&id).unwrap();
        assert!(matches!(&c.src, Source::File(p) if p == Path::new("/a/b.png")));
        assert_eq!((c.seq, c.target.as_deref()), (1, Some("访达")));
        assert!(rt.current("别的").is_none());
        // 换了一张：上一张的不认，按那一次关的也不关
        entered(&mut rt, 2, Entry::Other, "微信", Source::Pin { n: 1 });
        assert!(rt.current(&id).is_none());
        assert!(rt.shut_if(1, 2).is_none() && rt.open);
        assert_eq!(rt.shut_if(2, 2).unwrap().0.target.as_deref(), Some("微信"));
        assert!(matches!(rt.src, Source::Blank) && rt.cur.is_none());
        // 试用结束、空白窗口没有图
        rt.enter(3, Some(meme::NOWHERE), None, Source::Pin { n: 2 });
        assert!(rt.current(&id).is_none());
    }

    /// 窗口里 `⌘V`、最近的图是空白窗口来的，不算面板
    #[test]
    fn source_by_where_it_came_from() {
        let panel = || Source::Panel {
            record: 1,
            file: None,
        };
        assert!(matches!(kept(panel(), "blank"), Source::Blank));
        assert!(matches!(
            kept(panel(), "panel"),
            Source::Panel { record: 1, .. }
        ));
    }

    /// 撤遮罩之前先看：太大、读不出来的都报出来
    #[test]
    fn peek_before_leaving_overlay() {
        let state = AppState::for_test();
        let tmp = Tmp::new();
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        let shot = |png| peek(&state, &Source::Shot { rect, png });
        assert!(shot(png_head(40, 30)).is_ok());
        assert_eq!(msg(shot(png_head(20_000, 20_000))), "图太大，改不了");
        assert_eq!(msg(shot(b"x".to_vec())), "这张图读不出来");
        let f = tmp.0.join("a.png");
        assert_eq!(
            msg(peek(&state, &Source::File(f.clone()))),
            "这张图读不出来"
        );
        std::fs::write(&f, png_head(20_000, 20_000)).unwrap();
        assert_eq!(
            msg(peek(&state, &Source::File(f.clone()))),
            "图太大，改不了"
        );
        std::fs::write(&f, png_head(40, 30)).unwrap();
        assert!(peek(&state, &Source::File(f.clone())).is_ok());
        assert_eq!(dims(&f), Some((40, 30)));
        assert_eq!(msg(peek(&state, &Source::Clip { record: 999 })), UNUSABLE);
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
