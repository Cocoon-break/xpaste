//! 晾衣绳浮层窗口 `line` 与屏幕顶端检测（只在带 Pro 的构建里启动）。
//! 窗口启动时预建、隐藏；macOS 是不激活应用的 NSPanel（同划词卡片），前台应用不变。
//! 检测线程每 16 ms 采样光标：碰到边缘先只露绳子（peek），停 200 ms 才放下（左右两侧甩过去也放下）；总开关关着时停住不醒；
//! 放下后线程接着采样，只把光标进出便签区域（`line-pointer`）告诉页面，收起由页面决定（离开、Esc、快捷键挂上后停够时间）。

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::Thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use xpaste_core::LineSide;

use crate::error::AppError;
use crate::events::{
    LINE_DRAG, LINE_HANG_SELECTED, LINE_HIDE, LINE_POINTER, LINE_SHOW, LINE_TOAST, emit_to,
};
use crate::panel::{Rect, on_main};
use crate::state::AppState;

pub const LABEL: &str = "line";
pub const PAGE: &str = "pro/web/line.html";

/// 窗口放下时的方式，随 `line-show` 发给页面。
#[derive(Clone, Copy, PartialEq, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum How {
    /// 只露绳子，窗口仍穿透点击
    Peek,
    /// 鼠标放下；点到便签时页面调 `line_keyboard(true)` 再拿键盘
    Mouse,
    /// 快捷键挂上后亮一下，不拿键盘
    Flash,
    /// 键盘打开，拿键盘
    Kbd,
    /// 拖着东西到顶端
    Drag,
    /// 提醒到点：放下、不拿键盘，页面让到期的便签高亮；光标进来再离开、或一直没人碰 10 s 后收起
    Remind,
}

#[derive(PartialEq, Debug)]
pub enum EdgeEvent {
    Peek,
    Open(How),
    Leave,
}

/// 停在边缘这么久才放下
const DWELL_MS: u64 = 200;
/// 程序坞在同一侧且自动隐藏：鼠标停得久一点，免得和唤出程序坞抢
const DWELL_DOCK_MS: u64 = 400;
/// 离屏幕两端这么近不算（角落留给系统的热角；顶部还有苹果菜单和状态栏图标）
const CORNER: f64 = 40.0;
/// 左右两侧：朝边缘甩的速度（px/ms）达到它，到边缘就直接放下。顶部不认甩，平时常往菜单栏推鼠标
const FLING: f64 = 0.85;
const TICK: Duration = Duration::from_millis(16);
/// 屏幕矩形缓存这么久
const SCREENS_TTL: Duration = Duration::from_secs(1);
/// 顶部窗口的初始高度（逻辑像素）；页面渲染后经 `line_resize` 改
const H0: f64 = 240.0;
/// 左右两侧窗口的初始宽度（逻辑像素）
pub const COL_W: f64 = 200.0;
/// 改位置后等页面收完再放下，最多等这么久（比收起兜底长一点）
const RELOCATE_WAIT: Duration = Duration::from_millis(1500);
/// 发出 `line-hide` 后页面这么久还没调 `line_hidden`（页面卡住、崩了），后端自己收起
const HIDDEN_FALLBACK: Duration = Duration::from_millis(1000);

#[derive(Clone, Copy)]
enum Phase {
    Away,
    /// 到边缘的时刻
    Peek(u64),
    /// 已放下，或按着键作废了：离开边缘前不再发事件
    Open,
}

/// 采样时的鼠标主键
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Press {
    Up,
    /// 按着，但不是在拖东西（拖动窗口、框选、点菜单栏）
    Held,
    /// 按着在拖东西（文件、文字）
    Drag,
}

/// 按着键时算不算在拖东西。`base`、`now`：按下时和此刻拖拽剪切板的计数（macOS），变了才算；
/// 读不到（Windows）就只看系统是不是在拖动、缩放窗口（`moving`）。
pub fn press_of(base: Option<i64>, now: Option<i64>, moving: bool) -> Press {
    if moving || now.is_some_and(|n| base == Some(n)) {
        Press::Held
    } else {
        Press::Drag
    }
}

/// 边缘检测的状态机，见 `step`。
pub struct Edge {
    /// 上一次采样 (x, y, t)
    last: Option<(f64, f64, u64)>,
    phase: Phase,
    /// 上一次采样的鼠标主键
    prev: Press,
}

impl Default for Edge {
    fn default() -> Self {
        Self {
            last: None,
            phase: Phase::Away,
            prev: Press::Up,
        }
    }
}

impl Edge {
    /// 每 16 ms 采样一次：碰到边缘停 200 ms 或朝左右两侧甩过去才放下，两屏相接的边不算。
    /// 在边缘按着键却不是在拖东西（停着时点了菜单栏、拖窗口贴顶）就作废，离开边缘才再算。
    /// rects：全部屏幕的逻辑坐标矩形；press：鼠标主键，拖东西时放下为 Drag
    pub fn step(
        &mut self,
        c: (f64, f64),
        rects: &[Rect],
        t_ms: u64,
        press: Press,
        cfg: &EdgeCfg,
    ) -> Option<EdgeEvent> {
        let at = at_edge(c, rects, cfg);
        let hidden_dock = cfg
            .dock
            .as_ref()
            .is_some_and(|d| d.autohide && d.on(cfg.side));
        let dwell = if hidden_dock { DWELL_DOCK_MS } else { DWELL_MS };
        // 沿这一侧的法线方向算速度：左侧看 x 减小，右侧看 x 增大
        let fling = !hidden_dock
            && self.last.is_some_and(|(x, _, t)| {
                let d = match cfg.side {
                    LineSide::Top => return false,
                    LineSide::Left => x - c.0,
                    LineSide::Right => c.0 - x,
                };
                t_ms > t && d / (t_ms - t) as f64 >= FLING
            });
        self.last = Some((c.0, c.1, t_ms));
        // 已经停在边缘时才按下的，一定不是拖进来的（Windows 读不到拖拽剪切板，按着一律像在拖）
        let pressed_here = matches!(self.phase, Phase::Peek(_)) && self.prev == Press::Up;
        let held = press == Press::Held || (press != Press::Up && pressed_here);
        self.prev = press;
        let how = if press == Press::Drag {
            How::Drag
        } else {
            How::Mouse
        };
        match (self.phase, at) {
            (Phase::Away, false) => None,
            (Phase::Open, true) => None,
            (Phase::Open, false) => {
                self.phase = Phase::Away;
                None
            }
            (Phase::Peek(_), false) => {
                self.phase = Phase::Away;
                Some(EdgeEvent::Leave)
            }
            (Phase::Away, true) if held => {
                self.phase = Phase::Open;
                None
            }
            (Phase::Peek(_), true) if held => {
                self.phase = Phase::Open;
                Some(EdgeEvent::Leave)
            }
            (Phase::Away, true) if !fling => {
                self.phase = Phase::Peek(t_ms);
                Some(EdgeEvent::Peek)
            }
            (Phase::Peek(since), true) if !fling && t_ms - since < dwell => None,
            (_, true) => {
                self.phase = Phase::Open;
                Some(EdgeEvent::Open(how))
            }
        }
    }
}

fn contains(r: &Rect, (x, y): (f64, f64)) -> bool {
    x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DockSide {
    Left,
    Right,
    Bottom,
}

/// 系统的程序坞 / 任务栏：rect 与屏幕矩形同一坐标空间
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Dock {
    pub rect: Rect,
    pub side: DockSide,
    pub autohide: bool,
}

impl Dock {
    /// 和绳子挂在同一侧
    fn on(&self, side: LineSide) -> bool {
        matches!(
            (self.side, side),
            (DockSide::Left, LineSide::Left) | (DockSide::Right, LineSide::Right)
        )
    }
}

pub struct EdgeCfg {
    pub side: LineSide,
    pub dock: Option<Dock>,
}

/// 光标在绳子那一侧最外面一像素，且再往外没有别的屏（两屏相接的边不算）；离这条边两端 40 px 内不算。
/// 左右两侧另外：同侧程序坞常驻时，只认它上下露出来的那段。
pub fn at_edge(c: (f64, f64), rects: &[Rect], cfg: &EdgeCfg) -> bool {
    let Some(r) = rects.iter().find(|r| contains(r, c)) else {
        return false;
    };
    let beyond = match cfg.side {
        LineSide::Top => {
            return c.1 - r.y <= 1.0
                && c.0 - r.x >= CORNER
                && r.x + r.w - c.0 >= CORNER
                && !rects.iter().any(|o| contains(o, (c.0, r.y - 1.0)));
        }
        LineSide::Left if c.0 - r.x <= 1.0 => (r.x - 1.0, c.1),
        LineSide::Right if r.x + r.w - c.0 <= 1.0 => (r.x + r.w, c.1),
        _ => return false,
    };
    if c.1 - r.y < CORNER || r.y + r.h - c.1 < CORNER || rects.iter().any(|o| contains(o, beyond)) {
        return false;
    }
    // 横向也要落在程序坞那一截里（容 1 px），免得一块屏上的程序坞挡住另一块屏露在外面的边
    !cfg.dock.as_ref().is_some_and(|d| {
        d.on(cfg.side)
            && !d.autohide
            && c.1 >= d.rect.y
            && c.1 < d.rect.y + d.rect.h
            && c.0 >= d.rect.x - 1.0
            && c.0 <= d.rect.x + d.rect.w + 1.0
    })
}

/// 窗口在工作区里的矩形：顶部占工作区全宽、高为 `extent`；左右两侧占工作区全高、宽为 `extent`，贴着那一侧。
/// `extent` 不超过工作区的高（顶部）或宽（左右）。
pub fn side_rect(work: Rect, side: LineSide, extent: f64) -> Rect {
    match side {
        LineSide::Top => Rect {
            h: extent.min(work.h),
            ..work
        },
        LineSide::Left => Rect {
            w: extent.min(work.w),
            ..work
        },
        LineSide::Right => {
            let w = extent.min(work.w);
            Rect {
                x: work.x + work.w - w,
                w,
                ..work
            }
        }
    }
}

/// 提示里的叫法：Windows 上是任务栏
const DOCK_NAME: &str = if cfg!(target_os = "windows") {
    "任务栏"
} else {
    "程序坞"
};

/// 设置里“位置”下面的提示：程序坞在同一侧且自动隐藏，要停得久一点；常驻且上下露出的两段都不超过角落的 40 px，碰边放不下。
/// `screen` 是程序坞所在屏的矩形，与 `dock` 同一坐标空间。
pub fn dock_hint(side: LineSide, dock: Option<&Dock>, screen: Rect) -> String {
    let Some(d) = dock.filter(|d| d.on(side)) else {
        return String::new();
    };
    let at = if side == LineSide::Left {
        "左侧"
    } else {
        "右侧"
    };
    if d.autohide {
        return format!("{DOCK_NAME}也在{at}，鼠标停得久一点才会放下绳子");
    }
    let above = d.rect.y - screen.y;
    let below = screen.y + screen.h - (d.rect.y + d.rect.h);
    if above > CORNER || below > CORNER {
        return String::new();
    }
    format!("{DOCK_NAME}占满了{at}，用快捷键放下晾衣绳")
}

/// 能点的矩形四周再放宽这么多：光标贴着卡片边时不在穿透与接收之间来回切
const HIT_PAD: f64 = 4.0;

/// 常驻时光标落在页面报的能点的矩形里（窗口内逻辑像素，四周放宽 4 px）
pub fn hit_test(local: (f64, f64), hits: &[Rect]) -> bool {
    hits.iter().any(|r| {
        let r = Rect {
            x: r.x - HIT_PAD,
            y: r.y - HIT_PAD,
            w: r.w + 2.0 * HIT_PAD,
            h: r.h + 2.0 * HIT_PAD,
        };
        contains(&r, local)
    })
}

/// 常驻时窗口该不该穿透点击：`hit` 光标在能点的地方。按着键时保持按下那一刻的状态（拖便签拖出卡片不断，在空白处按下拖过卡片也不截走）；
/// `drag` 按着键碰到了边缘（拖文件过来），整窗接收到松开。`prev` 是上次定下的，刚放下时为 None。
pub fn pass_through(hit: bool, button: bool, drag: bool, prev: Option<bool>) -> bool {
    if drag {
        return false;
    }
    match prev {
        Some(p) if button => p,
        _ => !hit,
    }
}

/// 光标在另一块屏上（不是窗口所在的那块）；光标不在任何屏上不算。`win` 与 `rects`、`c` 同一坐标空间。
pub fn other_screen(c: (f64, f64), win: Rect, rects: &[Rect]) -> bool {
    rects
        .iter()
        .find(|r| contains(r, c))
        .is_some_and(|r| !contains(r, (win.x, win.y)))
}

/// 光标在便签区域里：晾衣绳窗口的矩形往绳子那一侧延到所在屏的边（顶部往上，菜单栏那一条也算；左右往屏幕边）。
/// `win` 与 `rects`、`c` 同一坐标空间。
pub fn pointer_inside(c: (f64, f64), win: Rect, rects: &[Rect], side: LineSide) -> bool {
    let scr = rects.iter().find(|r| contains(r, (win.x, win.y)));
    let (mut x0, mut x1, mut y0) = (win.x, win.x + win.w, win.y);
    match side {
        LineSide::Top => y0 = scr.map_or(win.y, |r| r.y),
        LineSide::Left => x0 = scr.map_or(win.x, |r| r.x),
        LineSide::Right => x1 = scr.map_or(x1, |r| r.x + r.w),
    }
    c.0 >= x0 && c.0 < x1 && c.1 >= y0 && c.1 < win.y + win.h
}

/// 窗口这一侧的状态。`how` 为 None：窗口已隐藏（页面调过 `line_hidden`）。
struct Shown {
    how: Option<How>,
    /// 所在屏的工作区（逻辑像素）、缩放与挂的位置，放下时记下
    work: Rect,
    sf: f64,
    side: LineSide,
    /// 页面报的横轴外的尺寸：顶部是高，左右两侧是宽
    extent: f64,
    keyboard: bool,
    /// Windows：拿键盘前的前台窗口，交还键盘时切回去（macOS 恒为 None，orderOut 时系统自己交还）
    previous: Option<isize>,
    /// 常驻时页面报的能点的矩形（窗口内逻辑像素），检测线程按它切换点击穿透
    hits: Vec<Rect>,
    /// 常驻的绳子在全屏应用上临时露出来了，这个时刻之后离开再让开
    temp: Option<Instant>,
    /// Windows：前台是全屏应用，常驻的窗口先撤下了（macOS 由空间规则让开，恒为 false）
    veiled: bool,
}

/// 这个构建带晾衣绳，且是官方签名的那份（被改过再重签的副本表现同开源版）：窗口、顶端检测、快捷键都看它
pub fn enabled() -> bool {
    xpaste_pro::AVAILABLE && xpaste_pro::genuine()
}

/// 用户开关、挂的位置与一直显示：启动和保存设置时写，检测线程与提醒线程读，不用锁 settings
static ON: AtomicBool = AtomicBool::new(true);
static SIDE: AtomicU8 = AtomicU8::new(0);
static PINNED: AtomicBool = AtomicBool::new(false);

/// 位置变了（含启动时读到的不是顶部）时，窗口尺寸换成新位置的初始值，等页面再报。不要在持有设置锁时调用。
pub fn configure(on: bool, side: LineSide, pinned: bool) {
    ON.store(on, Ordering::Relaxed);
    PINNED.store(pinned, Ordering::Relaxed);
    if SIDE.swap(side as u8, Ordering::Relaxed) != side as u8 {
        shown().extent = initial_extent(side);
    }
    wake_detect();
}

/// 检测线程：页面没就绪或总开关关着时停住，由 `ready`、`configure` 叫醒
static DETECT: OnceLock<Thread> = OnceLock::new();

fn wake_detect() {
    if let Some(t) = DETECT.get() {
        t.unpark();
    }
}

/// 新位置的窗口初始尺寸：横挂是高，竖挂是宽
fn initial_extent(side: LineSide) -> f64 {
    if side == LineSide::Top { H0 } else { COL_W }
}

/// 构建带晾衣绳且用户没关掉：检测边缘、触发提醒、接住拖动都看它
pub fn active() -> bool {
    enabled() && ON.load(Ordering::Relaxed)
}

/// 一直显示：总开关开着且选了一直显示。这时绳子不收起（离开、Esc、亮一下到时、提醒到时都不收）
pub fn pinned() -> bool {
    active() && PINNED.load(Ordering::Relaxed)
}

pub fn side() -> LineSide {
    match SIDE.load(Ordering::Relaxed) {
        1 => LineSide::Left,
        2 => LineSide::Right,
        _ => LineSide::Top,
    }
}

static SHOWN: Mutex<Shown> = Mutex::new(Shown {
    how: None,
    work: Rect {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    },
    sf: 1.0,
    side: LineSide::Top,
    extent: H0,
    keyboard: false,
    previous: None,
    hits: Vec::new(),
    temp: None,
    veiled: false,
});

fn shown() -> MutexGuard<'static, Shown> {
    SHOWN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 页面已开始监听 `line-show`；之前检测线程只睡，不检测顶端（页面没加载好时放下的窗口会挡住顶端的点击）
static READY: AtomicBool = AtomicBool::new(false);

/// 页面开始监听 line-show 后调用；之前不检测顶端
pub fn ready(app: &AppHandle) {
    READY.store(true, Ordering::Relaxed);
    wake_detect();
    log::info!("晾衣绳页面就绪，开始检测顶端");
    // 关着应用时错过的提醒现在补发
    crate::remind::wake();
    if pinned() {
        show_pinned(app);
    }
}

/// 页面已就绪：之前提醒线程不放下绳子
pub fn is_ready() -> bool {
    READY.load(Ordering::Relaxed)
}

/// 绳子已放下（只露绳子不算）时窗口的矩形，换到检测线程的坐标空间（除以 `k` 之后），以及从窗口逻辑像素换过去的倍数；没放下为 None。
/// macOS 两边都是点；Windows 窗口按所在屏缩放记的逻辑像素，检测线程按主屏缩放，差一个 sf / k。
fn open_rect(k: f64) -> Option<(Rect, f64)> {
    let s = shown();
    s.how.filter(|&h| h != How::Peek)?;
    let f = if cfg!(target_os = "macos") {
        1.0
    } else {
        s.sf / k
    };
    let r = side_rect(s.work, s.side, s.extent);
    let win = Rect {
        x: r.x * f,
        y: r.y * f,
        w: r.w * f,
        h: r.h * f,
    };
    Some((win, f))
}

/// 每次放下、收起加一：收起兜底只在这期间没再放下、也没收完时生效
static GEN: AtomicU64 = AtomicU64::new(0);

/// 预建窗口，起检测线程和提醒线程。只在带 Pro 的构建里调用。
pub fn start(app: &AppHandle) {
    prewarm(app);
    crate::remind::start(app);
    let app = app.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("line-edge".into())
        .spawn(move || detect(&app))
    {
        log::warn!("晾衣绳检测线程启动失败：{e}");
    }
}

/// `line-toast` 的载荷
#[derive(Clone, Serialize, PartialEq, Debug)]
pub struct Toast {
    pub text: String,
    /// `"pro"`：提示条带“输入授权码”按钮
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<&'static str>,
    /// 已挂着的那张便签
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
}

/// 快捷键没挂成时的提示；没有记录时 `err` 为 None。
fn toast_for(err: Option<&AppError>) -> Toast {
    let text = err.map_or_else(|| "剪切板里还没有内容".to_owned(), ToString::to_string);
    Toast {
        text,
        action: matches!(err, Some(AppError::LineExpired)).then_some("pro"),
        id: match err {
            Some(AppError::LineDuplicate(id)) => Some(*id),
            _ => None,
        },
    }
}

/// 便签快捷键按下（读库、写便签文件等重活都在单独的线程里做，不占主线程）：面板可见 → 让面板挂它选中的记录；否则挂最新一条。
/// 结果怎么显示见 `hang_and_flash`。
pub fn hotkey_pressed(app: &AppHandle) {
    let visible = app
        .get_webview_window("panel")
        .is_some_and(|w| w.is_visible().unwrap_or(false));
    if visible {
        // 只发给面板：隐藏的晾衣绳窗口可能也载入了面板前端
        emit_to(app, "panel", LINE_HANG_SELECTED, ());
        return;
    }
    let app = app.clone();
    // 读库与写便签文件，不占着快捷键回调所在的主线程
    if let Err(e) = std::thread::Builder::new()
        .name("line-hotkey".into())
        .spawn(move || hang_and_flash(&app, None, true))
    {
        log::warn!("便签快捷键线程启动失败：{e}");
    }
}

/// 快捷键挂的已经挂着、绳子也放下着（只露一截不算）：改成键盘操作并选中那张，返回它的 id；否则 None，照常亮一下并提示。
/// 面板里挂的（`select` 为 false）从不选中：拿键盘会让面板失焦隐藏。
fn select_on_duplicate(shown: Option<How>, err: Option<&AppError>, select: bool) -> Option<u64> {
    match err {
        Some(AppError::LineDuplicate(id)) if select && shown.is_some_and(|h| h != How::Peek) => {
            Some(*id)
        }
        _ => None,
    }
}

/// 快捷键、挂钩按钮挂的插在哪：顶部挂在末尾，左右两侧挂在最上面
fn hang_at(side: LineSide) -> Option<usize> {
    (side != LineSide::Top).then_some(0)
}

/// 挂上记录 `id`（None 为最新一条）：挂上了就放下绳子亮一下（已放下时续亮）；
/// 已经挂着时，`select`（快捷键）且绳子放下着就改成键盘操作并选中那张，否则亮一下、提示“已经挂着了”并让那张轻摆；
/// 其余没挂成的亮一下并提示。便签快捷键与面板的挂钩按钮共用；要读库、写便签文件，不在主线程调用。
pub fn hang_and_flash(app: &AppHandle, id: Option<i64>, select: bool) {
    let state = app.state::<AppState>();
    let id = match id {
        Some(id) => Ok(Some(id)),
        None => state.store().latest_live_id(),
    };
    // Err(None)：剪切板里还没有记录
    let res = match id {
        Ok(Some(id)) => crate::commands::line::hang_record(app, &state, id, hang_at(side()))
            .map(|n| n.truncated)
            .map_err(Some),
        Ok(None) => Err(None),
        Err(e) => {
            log::warn!("读取最新记录失败：{e}");
            Err(Some(e.into()))
        }
    };
    let truncated = matches!(res, Ok(true));
    let err = res.err();
    let how = shown().how;
    if let Some(sel) = select_on_duplicate(how, err.as_ref().and_then(Option::as_ref), select) {
        return show_selecting(app, How::Kbd, Some(sel));
    }
    show(app, How::Flash);
    if let Some(e) = err {
        emit_to(app, LABEL, LINE_TOAST, toast_for(e.as_ref()));
    } else if truncated {
        let text = crate::commands::line::TRUNCATED_HINT.to_owned();
        emit_to(
            app,
            LABEL,
            LINE_TOAST,
            Toast {
                text,
                action: None,
                id: None,
            },
        );
    }
}

/// 面板里拖着的那条记录（取自面板的列表数据），晾衣绳页面画跟手的影子便签用
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DragItem {
    pub id: i64,
    pub kind: xpaste_core::Kind,
    pub title: String,
    /// 图片、视频缩略图的 asset URL
    pub thumb_url: Option<String>,
}

/// `line-drag` 的载荷：光标的屏幕坐标加拖着的记录
#[derive(Clone, Serialize)]
struct DragHover {
    x: f64,
    y: f64,
    #[serde(flatten)]
    item: DragItem,
}

/// 面板里拖着行到了顶端：绳子还没为拖动放下就放下，再把光标位置和记录转给页面。
/// `show` 在主线程里才记下 `how`，紧接着的几帧可能再放下一次，页面只多收到几次同样的 `line-show`。
pub fn drag_hover(app: &AppHandle, x: f64, y: f64, item: DragItem) {
    if !active() {
        return;
    }
    if shown().how != Some(How::Drag) {
        show(app, How::Drag);
    }
    emit_to(app, LABEL, LINE_DRAG, DragHover { x, y, item });
}

/// 建好隐藏的窗口，免得碰到顶端时现建 webview。参数同划词卡片。
fn prewarm(app: &AppHandle) {
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(PAGE.into()))
        .title("xpaste-line")
        .theme(crate::panel::current_theme(app))
        .visible(false)
        .focused(false)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .resizable(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .accept_first_mouse(true)
        .inner_size(800.0, H0)
        // 同面板：隐藏久了 WebKit 挂起页面进程，放下时先空白零点几秒
        .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
        .build();
    match built {
        #[cfg(target_os = "macos")]
        Ok(w) => on_main(app, move |_| {
            // 层级同划词卡片（NSFloatingWindowLevel）
            if let Err(e) = crate::shot::to_overlay_panel(&w, 3) {
                log::warn!("晾衣绳窗口转换 NSPanel 失败：{e}");
            }
        }),
        #[cfg(not(target_os = "macos"))]
        Ok(_) => {}
        Err(e) => log::warn!("创建晾衣绳窗口失败：{e}"),
    }
}

/// 常驻时检测线程自己记的状态
#[derive(Default)]
struct Pin {
    /// 上次设下的点击穿透；放下、收起后（GEN 变了）重新设，按着键时等松开再重新设
    pass: Option<bool>,
    generation: u64,
    /// GEN 变了、还没按光标重新设过点击穿透（按着键）
    stale: bool,
    /// 按着键碰到了边缘（拖文件过来）：整窗接收到松开
    drag: bool,
    /// 光标离开便签区域的时刻
    out: Option<Instant>,
    /// Windows：前台是全屏应用（每秒随屏幕矩形读一次）
    fullscreen: bool,
}

impl Pin {
    /// 放下、收起后（GEN 变了）忘掉上次设下的点击穿透，按光标重新设；按着键时（拖便签、甩绳子中途来了提醒、快捷键）
    /// 先保持，松开再重新设，免得拖到一半窗口变成点穿
    fn regen(&mut self, generation: u64, button: bool) {
        if generation != self.generation {
            self.generation = generation;
            self.stale = true;
        }
        if self.stale && !button {
            self.stale = false;
            self.pass = None;
        }
    }
}

/// 离开便签区域这么久，临时露出来的常驻绳子再让开（同页面离开后收起）
const LEAVE: Duration = Duration::from_millis(350);

/// 常驻时碰到边缘放下（`how`）：碰到别的屏的边就挪过去留在那里，碰到本屏的边在全屏应用上临时露出来。
/// 按着键碰到的（拖文件过来）整窗接收到松开。
fn pin_edge(app: &AppHandle, pin: &mut Pin, how: How, other: bool) {
    pin.drag = how == How::Drag;
    if other {
        log::info!("晾衣绳常驻挪到另一块屏");
    }
    on_main(app, move |app| {
        let Some(w) = app.get_webview_window(LABEL) else {
            return;
        };
        if other && let Err(e) = place(app, &w, false) {
            log::warn!("晾衣绳定位失败：{e}");
        }
        // 挪过去的那块屏也可能正全屏着
        reveal(app, &w, Duration::ZERO);
    });
}

/// 常驻时每次采样：按光标在不在能点的地方切换点击穿透；临时露出来的离开 350 ms 后再让开；Windows 进出全屏应用时撤下、放回。
/// `local` 是光标在窗口里的逻辑坐标，`inside` 光标在便签区域里。
fn pin_step(app: &AppHandle, pin: &mut Pin, local: (f64, f64), inside: bool, button: bool) {
    if !button {
        pin.drag = false;
    }
    let (hit, keyboard, temp, veiled) = {
        let s = shown();
        (hit_test(local, &s.hits), s.keyboard, s.temp, s.veiled)
    };
    pin.regen(GEN.load(Ordering::Relaxed), button);
    let want = pass_through(hit, button, pin.drag, pin.pass);
    if pin.pass != Some(want) {
        // 刚从接收换成穿透：页面收不到光标离开，告诉它把悬停的样子去掉
        if want && pin.pass == Some(false) {
            emit_to(
                app,
                LABEL,
                LINE_POINTER,
                serde_json::json!({ "inside": inside, "pass": true }),
            );
        }
        pin.pass = Some(want);
        on_main(app, move |app| {
            if let Some(w) = app.get_webview_window(LABEL)
                && let Err(e) = w.set_ignore_cursor_events(want)
            {
                log::warn!("晾衣绳设置点击穿透失败：{e}");
            }
        });
    }
    pin.out = if inside {
        None
    } else {
        pin.out.or_else(|| Some(Instant::now()))
    };
    let left = pin.out.is_some_and(|o| o.elapsed() >= LEAVE);
    if let Some(until) = temp {
        if left && !keyboard && !button && Instant::now() >= until {
            veil(app, pin.fullscreen);
        }
    } else if pin.fullscreen != veiled {
        conceal(app, pin.fullscreen);
    }
}

/// 试用已结束且绳上没有便签：碰边缘不再放下，免得没用过的人一碰顶端就看到到期提示；快捷键、挂钩按钮照常
fn muted(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let line = state.line();
    line.notes().is_empty()
        && matches!(
            line.status(crate::clock::now_ms() / 1000),
            xpaste_pro::Pro::Expired { .. }
        )
}

/// 拖拽剪切板的基准计数：松着键时记（松开那一刻、线程刚起或醒来后第一次），下一次按下就一定在它之后，见 `press_of`
#[derive(Default)]
struct PressProbe {
    base: Option<i64>,
    down: bool,
}

impl PressProbe {
    /// `button` 主键按着，`at` 光标在边缘；`count`、`moving` 同平台的 `drag_count`、`moving_window`。
    /// 按着却不在边缘时一律报 Held，不读（只有在边缘才分是不是在拖）。
    // ponytail: 拖放结束后源应用要是松开后才往拖拽剪切板写，下一次普通按下会当成在拖；真机碰到再看
    fn sample(
        &mut self,
        button: bool,
        at: bool,
        count: impl Fn() -> Option<i64>,
        moving: impl Fn() -> bool,
    ) -> Press {
        if !button {
            if self.down || self.base.is_none() {
                self.base = count();
            }
            self.down = false;
            return Press::Up;
        }
        self.down = true;
        if !at {
            return Press::Held;
        }
        press_of(self.base, count(), moving())
    }
}

/// 检测线程：每 16 ms 采样光标，屏幕矩形缓存 1 s；绳子放下期间只报光标进出便签区域（常驻时另见 `pin_step`）。
fn detect(app: &AppHandle) {
    let _ = DETECT.set(std::thread::current());
    let t0 = Instant::now();
    let mut edge = Edge::default();
    let mut screens = (Vec::new(), 1.0);
    let mut dock = None;
    let mut read_at: Option<Instant> = None;
    // 上次报给页面的进出；每次放下后先报一次
    let mut inside: Option<bool> = None;
    let mut pin = Pin::default();
    let mut probe = PressProbe::default();
    let mut quiet = false;
    loop {
        std::thread::sleep(TICK);
        // 页面没就绪、用户关掉后不碰光标、不读屏幕，停到被叫醒（park 也可能无故返回，回到开头再看）
        if !READY.load(Ordering::Relaxed) || !active() {
            edge = Edge::default();
            inside = None;
            std::thread::park();
            // 停着期间可能拖过东西，醒来重新记基准
            probe = PressProbe::default();
            continue;
        }
        if read_at.is_none_or(|t| t.elapsed() >= SCREENS_TTL) {
            read_at = Some(Instant::now());
            match read_screens(app) {
                Ok(s) => screens = s,
                Err(e) => log::debug!("晾衣绳读取屏幕失败：{e}"),
            }
            // 顶部用不到程序坞：不读，免得每秒访问一次别的进程
            dock = (side() != LineSide::Top)
                .then(|| read_dock(screens.1))
                .flatten();
            // ponytail: 跟着屏幕矩形每秒读一次，进全屏后最多晚 1 s 让开；嫌慢再单独加快
            pin.fullscreen = pinned() && xpaste_platform::fullscreen_app();
            quiet = muted(app);
        }
        // 不用 app.cursor_position()：它要等主线程应答，每秒 60 次会一直唤醒主线程
        let Some((x, y)) = xpaste_platform::pointer_position() else {
            continue;
        };
        let (rects, k) = (&screens.0, screens.1);
        let cfg = EdgeCfg { side: side(), dock };
        let c = (x / k, y / k);
        // 放下期间也跟着按下、松开，基准才是这次按下之前的
        let press = probe.sample(
            xpaste_platform::mouse_button_down(),
            at_edge(c, rects, &cfg),
            xpaste_platform::drag_count,
            xpaste_platform::moving_window,
        );
        if let Some(open) = open_rect(k) {
            let now_in = pointer_inside(c, open.0, rects, cfg.side);
            if inside != Some(now_in) {
                inside = Some(now_in);
                emit_to(
                    app,
                    LABEL,
                    LINE_POINTER,
                    serde_json::json!({ "inside": now_in }),
                );
            }
            if pinned() {
                let t = t0.elapsed().as_millis() as u64;
                let (win, f) = open;
                if let Some(EdgeEvent::Open(how)) = edge.step(c, rects, t, press, &cfg) {
                    pin_edge(app, &mut pin, how, other_screen(c, win, rects));
                }
                let local = ((c.0 - win.x) / f, (c.1 - win.y) / f);
                pin_step(app, &mut pin, local, now_in, press != Press::Up);
                continue;
            }
            // 收起后先离开边缘才会再放下：按 Esc 收起时光标还在顶端，不立刻又放下来
            edge = Edge {
                phase: Phase::Open,
                ..Edge::default()
            };
            continue;
        }
        inside = None;
        // 试用结束且绳上没便签：不再开始露绳子，已经露出来的照常走完；拖着东西过来的照常（同面板里拖记录，放下后看到到期提示）
        if quiet && matches!(edge.phase, Phase::Away) && press != Press::Drag {
            continue;
        }
        let t = t0.elapsed().as_millis() as u64;
        match edge.step(c, rects, t, press, &cfg) {
            Some(EdgeEvent::Peek) => show(app, How::Peek),
            Some(EdgeEvent::Open(how)) => show(app, how),
            Some(EdgeEvent::Leave) => hide(app),
            None => {}
        }
    }
}

/// 全部屏幕的矩形，以及 `pointer_position` 要除的系数，除完与矩形同一空间：
/// macOS 的指针已是点（主屏左上为原点），显示器按各自缩放换回点，系数为 1；
/// Windows 的指针与显示器都是物理像素（Per-Monitor-v2），统一除以主屏缩放：混合 DPI 下屏与屏相接的关系不变，
/// 主屏上的单位就是逻辑像素（顶端 1 px、甩的速度都按它算）。
fn read_screens(app: &AppHandle) -> tauri::Result<(Vec<Rect>, f64)> {
    let psf = if cfg!(target_os = "macos") {
        1.0
    } else {
        app.primary_monitor()?.map_or(1.0, |m| m.scale_factor())
    };
    let rects = app
        .available_monitors()?
        .iter()
        .map(|m| {
            let k = if cfg!(target_os = "macos") {
                m.scale_factor()
            } else {
                psf
            };
            let (p, s) = (m.position(), m.size());
            Rect {
                x: p.x as f64 / k,
                y: p.y as f64 / k,
                w: s.width as f64 / k,
                h: s.height as f64 / k,
            }
        })
        .collect();
    Ok((rects, psf))
}

/// 程序坞 / 任务栏，换到检测线程的坐标空间（除以 `k`，同指针）；读不到为 None
fn read_dock(k: f64) -> Option<Dock> {
    let Some(d) = xpaste_platform::dock() else {
        log::debug!("晾衣绳读不到程序坞位置");
        return None;
    };
    use xpaste_platform::DockEdge;
    Some(Dock {
        rect: Rect {
            x: d.x / k,
            y: d.y / k,
            w: d.w / k,
            h: d.h / k,
        },
        side: match d.edge {
            DockEdge::Left => DockSide::Left,
            DockEdge::Right => DockSide::Right,
            DockEdge::Bottom => DockSide::Bottom,
        },
        autohide: d.autohide,
    })
}

/// 设置里“位置”下面的程序坞提示，见 `dock_hint`；程序坞所在屏按它中心所在的那块屏算。读程序坞要访问别的进程，不在主线程调用。
pub fn side_hint(app: &AppHandle, side: LineSide) -> String {
    let (rects, k) = match read_screens(app) {
        Ok(s) => s,
        Err(e) => {
            log::debug!("晾衣绳读取屏幕失败：{e}");
            return String::new();
        }
    };
    let Some(d) = read_dock(k) else {
        return String::new();
    };
    let mid = (d.rect.x + d.rect.w / 2.0, d.rect.y + d.rect.h / 2.0);
    rects
        .iter()
        .find(|r| contains(r, mid))
        .map_or_else(String::new, |&r| dock_hint(side, Some(&d), r))
}

/// 放到光标所在屏工作区顶端，宽为工作区宽，发 `line-show { how }`。
/// 已放下时不退回只露绳子；只露绳子之后再放下只换 `how`，窗口不挪。`Kbd` 拿键盘，其余不拿。
pub fn show(app: &AppHandle, how: How) {
    show_selecting(app, how, None);
}

/// 同 `show`，`line-show` 另带要选中的便签 `id`（键盘落在它上面）。
fn show_selecting(app: &AppHandle, how: How, select: Option<u64>) {
    show_on(app, how, select, false);
}

/// 一直显示：放到主屏（有菜单栏的那块）上，已放下着也挪过去；不拿键盘
pub fn show_pinned(app: &AppHandle) {
    show_on(app, How::Mouse, None, true);
}

/// 设置里改了“一直显示”或打开了总开关：常驻就放下，否则收起
pub fn repin(app: &AppHandle) {
    if pinned() {
        show_pinned(app);
    } else {
        hide(app);
    }
}

/// `primary`：放到主屏（放下着也挪过去）；否则第一次放下时放到光标所在屏。
fn show_on(app: &AppHandle, how: How, select: Option<u64>, primary: bool) {
    // 常驻时没有只露绳子这一步
    let how = if how == How::Peek && pinned() {
        How::Mouse
    } else {
        how
    };
    log::info!("晾衣绳放下 {how:?}");
    on_main(app, move |app| {
        let Some(w) = app.get_webview_window(LABEL) else {
            return log::warn!("找不到晾衣绳窗口");
        };
        let (first, temp) = {
            let mut s = shown();
            if how == How::Peek && s.how.is_some_and(|h| h != How::Peek) {
                return;
            }
            let first = s.how.is_none();
            s.how = Some(how);
            GEN.fetch_add(1, Ordering::Relaxed);
            (first, s.temp.is_some())
        };
        // 常驻（primary）时放下着也挪到主屏
        if (first || primary)
            && let Err(e) = place(app, &w, primary)
        {
            log::warn!("晾衣绳定位失败：{e}");
        }
        let pin = pinned();
        // 只露绳子时点击穿透到下面的应用；常驻时刚放下先穿透，检测线程按光标在不在能点的地方切换。
        // 常驻且已放下着（提醒、快捷键、键盘又放下一次）不动它，交给检测线程，免得拖到一半变成点穿
        if !(pin && !first)
            && let Err(e) = w.set_ignore_cursor_events(how == How::Peek || pin)
        {
            log::warn!("晾衣绳设置点击穿透失败：{e}");
        }
        // 常驻的不进别的应用的全屏空间；临时露出来的、平时放下的照旧
        spaces(app, !pin || temp);
        emit_to(
            app,
            LABEL,
            LINE_SHOW,
            serde_json::json!({ "how": how, "id": select }),
        );
        // 常驻的绳子被全屏应用挡着时，快捷键、键盘、提醒、拖进来也要看得见：临时露出来
        if pin && !first && how != How::Mouse {
            let hold = match how {
                How::Flash => Duration::from_secs(2),
                How::Remind => Duration::from_secs(10),
                _ => Duration::ZERO,
            };
            reveal(app, &w, hold);
        }
        let r = if how == How::Kbd {
            take_keyboard(app, &w)
        } else if first {
            front(app, &w, false)
        } else {
            Ok(())
        };
        if let Err(e) = r {
            log::warn!("显示晾衣绳失败：{e}");
        }
    });
}

/// 上屏，`focus` 时拿键盘。Windows 不拿键盘时用 `show_no_activate`，前台窗口不变。主线程调用。
fn front(app: &AppHandle, w: &WebviewWindow, focus: bool) -> tauri::Result<()> {
    crate::trcard::front(app, w, focus)
}

fn take_keyboard(app: &AppHandle, w: &WebviewWindow) -> tauri::Result<()> {
    log::info!(
        "晾衣绳拿键盘，面板可见 {}",
        crate::panel::window(app).is_some_and(|p| p.is_visible().unwrap_or(false))
    );
    {
        let mut s = shown();
        if !s.keyboard {
            s.keyboard = true;
            s.previous = xpaste_platform::foreground_window();
        }
    }
    front(app, w, true)
}

/// 光标所在屏（`primary` 时主屏）的工作区（避开菜单栏、程序坞、任务栏）里，按挂的位置贴着顶端或左右一侧。主线程调用。
fn place(app: &AppHandle, w: &WebviewWindow, primary: bool) -> tauri::Result<()> {
    let m = if primary {
        app.primary_monitor()?
    } else {
        crate::panel::cursor_monitor(app)?
    };
    let Some(m) = m else {
        return Ok(());
    };
    let (sf, wa) = (m.scale_factor(), m.work_area());
    let side = side();
    let mut work = Rect {
        x: wa.position.x as f64 / sf,
        y: wa.position.y as f64 / sf,
        w: wa.size.width as f64 / sf,
        h: wa.size.height as f64 / sf,
    };
    // 刘海在顶端正中，只挡顶部那一排
    if side == LineSide::Top {
        let screen_top = m.position().y as f64 / sf;
        let notch =
            xpaste_platform::notch_inset(m.position().x as f64 / sf, m.size().width as f64 / sf);
        work = below_notch(work, screen_top, notch);
    }
    let extent = {
        let mut s = shown();
        s.work = work;
        s.sf = sf;
        s.side = side;
        s.extent
    };
    apply(w, side_rect(work, side, extent), sf)
}

/// 工作区顶端高于刘海下沿时（菜单栏自动隐藏、全屏应用），往下让到刘海下面，免得中间的便签被刘海盖住。
pub fn below_notch(work: Rect, screen_top: f64, notch: f64) -> Rect {
    let top = screen_top + notch;
    if work.y >= top {
        return work;
    }
    Rect {
        y: top,
        h: (work.h - (top - work.y)).max(0.0),
        ..work
    }
}

/// 摆窗口到 `r`（逻辑像素）：macOS 用逻辑坐标（点），Windows 按所在屏缩放换成物理像素（同划词卡片）。
fn apply(w: &WebviewWindow, r: Rect, sf: f64) -> tauri::Result<()> {
    let k = if cfg!(target_os = "macos") { 1.0 } else { sf };
    crate::shot::place(w, (r.x * k, r.y * k), (r.w * k, r.h * k))
}

/// 发 `line-hide`；页面收起后调 `line_hidden`，窗口才真正隐藏。一直显示时不收（关掉一直显示或总开关后才收）。
pub fn hide(app: &AppHandle) {
    if pinned() {
        return;
    }
    hide_now(app);
}

/// 同 `hide`，一直显示时也收（改位置要先收起再到新位置放下）。
/// 1 s 内页面没调 `line_hidden`、其间也没再放下，就自己收起，免得页面卡住时窗口一直挡在顶端。
fn hide_now(app: &AppHandle) {
    if shown().how.is_none() {
        return;
    }
    emit_to(app, LABEL, LINE_HIDE, ());
    let g = GEN.fetch_add(1, Ordering::Relaxed) + 1;
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("line-hide".into())
        .spawn(move || {
            std::thread::sleep(HIDDEN_FALLBACK);
            // 判断与清空在同一把锁里：中间插进来的 show 会先加 GEN
            let fire = {
                let mut s = shown();
                let fire = GEN.load(Ordering::Relaxed) == g && s.how.is_some();
                if fire {
                    s.how = None;
                }
                fire
            };
            if fire {
                log::warn!("晾衣绳页面没有报告收完，直接收起");
                hidden(&app);
            }
        });
    if let Err(e) = spawned {
        log::warn!("晾衣绳收起兜底线程启动失败：{e}");
    }
}

/// 页面播完收起：撤下窗口、恢复点击穿透，拿过键盘时交还；之后检测线程继续采样。
pub fn hidden(app: &AppHandle) {
    log::info!("晾衣绳收起");
    let back = {
        let mut s = shown();
        s.how = None;
        s.keyboard = false;
        s.temp = None;
        s.veiled = false;
        s.previous.take()
    };
    on_main(app, move |app| {
        // Windows：窗口还在前台时先切回原来的窗口（本进程此刻是前台进程，系统允许），再撤下
        if let Some(h) = back {
            xpaste_platform::activate_window(h);
        }
        let Some(w) = app.get_webview_window(LABEL) else {
            return;
        };
        if let Err(e) = w.set_ignore_cursor_events(true) {
            log::warn!("晾衣绳设置点击穿透失败：{e}");
        }
        take_down(&w);
    });
}

/// 撤下窗口。NSPanel 上 hide 即 orderOut，键盘交还仍处于激活状态的前台应用。主线程调用。
fn take_down(w: &WebviewWindow) {
    if let Err(e) = w.hide() {
        log::warn!("隐藏晾衣绳失败：{e}");
    }
    // Windows：经 show_no_activate 上屏时 tao 的标记仍是隐藏，上面的 hide() 不会真的隐藏
    #[cfg(windows)]
    match w.hwnd() {
        Ok(h) => xpaste_platform::hide_window(h.0 as isize),
        Err(e) => log::warn!("晾衣绳取不到窗口句柄：{e}"),
    }
}

/// macOS：`aux` 为 false 时去掉 FullScreenAuxiliary，窗口照样出现在每个普通桌面上，但不进别的应用的全屏空间
/// （常驻时不盖在全屏视频、演示上面）；平时放下和临时露出来时带上。其余同 `shot::to_overlay_panel`。主线程调用。
#[allow(deprecated)] // cocoa 已弃用，但 tauri-nspanel 公开的就是它的类型
fn spaces(app: &AppHandle, aux: bool) {
    #[cfg(target_os = "macos")]
    {
        use tauri_nspanel::ManagerExt;
        use tauri_nspanel::cocoa::appkit::NSWindowCollectionBehavior as B;
        if let Ok(p) = app.get_webview_panel(LABEL) {
            let mut b = B::NSWindowCollectionBehaviorCanJoinAllSpaces
                | B::NSWindowCollectionBehaviorStationary;
            if aux {
                b |= B::NSWindowCollectionBehaviorFullScreenAuxiliary;
            }
            p.set_collection_behaviour(b);
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, aux);
}

/// 窗口此刻在屏幕上看得见。macOS 看遮挡状态：不带 FullScreenAuxiliary 的窗口在全屏空间里不显示，算看不见。
#[cfg(target_os = "macos")]
// 同 panel::animate_alpha：cocoa 已弃用；objc 0.2 的 sel! 展开里有 cfg(feature = "cargo-clippy")
#[allow(deprecated, unexpected_cfgs)]
fn on_screen(w: &WebviewWindow) -> bool {
    use tauri_nspanel::cocoa::base::id;
    use tauri_nspanel::objc::{msg_send, sel, sel_impl};
    let Ok(ns) = w.ns_window() else {
        return true;
    };
    // SAFETY: 主线程上对自己的 NSWindow 只读查询
    let state: usize = unsafe { msg_send![ns as id, occlusionState] };
    // NSWindowOcclusionStateVisible
    state & (1 << 1) != 0
}

/// Windows 上看不见只会是检测线程因全屏撤下的（`veiled`）
#[cfg(not(target_os = "macos"))]
fn on_screen(_: &WebviewWindow) -> bool {
    true
}

/// 常驻的绳子被别的应用全屏挡着（macOS 在全屏空间里、Windows 撤下了）时临时露出来；`hold` 之前、光标在里面、拿着键盘、按着键都不让开。主线程调用。
fn reveal(app: &AppHandle, w: &WebviewWindow, hold: Duration) {
    let veiled = std::mem::take(&mut shown().veiled);
    if !veiled && on_screen(w) {
        return;
    }
    log::info!("晾衣绳常驻在全屏应用上临时露出来");
    shown().temp = Some(Instant::now() + hold);
    spaces(app, true);
    if let Err(e) = front(app, w, false) {
        log::warn!("显示晾衣绳失败：{e}");
    }
}

/// 临时露出来的常驻绳子再让开：macOS 去掉 FullScreenAuxiliary 后撤下再上屏，系统按新的规则把它从全屏空间里拿走，普通桌面上它还在；
/// Windows 前台还是全屏应用时撤下。
fn veil(app: &AppHandle, fullscreen: bool) {
    shown().temp = None;
    on_main(app, move |app| {
        let Some(w) = app.get_webview_window(LABEL) else {
            return;
        };
        // 其间收起了或不再常驻：不再动它（下次放下时按平时的规则摆）
        if shown().how.is_none() || !pinned() {
            return;
        }
        if cfg!(target_os = "macos") {
            spaces(app, false);
            take_down(&w);
            if let Err(e) = front(app, &w, false) {
                log::warn!("显示晾衣绳失败：{e}");
            }
        } else if fullscreen {
            shown().veiled = true;
            take_down(&w);
        }
    });
}

/// Windows：前台进出全屏应用时撤下、放回常驻的窗口（临时露出来的由 `veil` 管）
fn conceal(app: &AppHandle, fullscreen: bool) {
    on_main(app, move |app| {
        let Some(w) = app.get_webview_window(LABEL) else {
            return;
        };
        {
            let mut s = shown();
            if s.how.is_none() || !pinned() || s.temp.is_some() || s.veiled == fullscreen {
                return;
            }
            s.veiled = fullscreen;
        }
        log::info!(
            "晾衣绳常驻{}",
            if fullscreen {
                "让开全屏应用"
            } else {
                "放回"
            }
        );
        if fullscreen {
            take_down(&w);
        } else if let Err(e) = front(app, &w, false) {
            log::warn!("显示晾衣绳失败：{e}");
        }
    });
}

/// 页面报常驻时能点的矩形（窗口内逻辑像素）；不是有限数、不是正尺寸的丢掉
pub fn set_hits(rects: Vec<[f64; 4]>) {
    shown().hits = rects
        .into_iter()
        .filter(|r| r.iter().all(|v| v.is_finite()) && r[2] > 0.0 && r[3] > 0.0)
        .map(|[x, y, w, h]| Rect { x, y, w, h })
        .collect();
}

/// 页面按内容改窗口尺寸：顶部改高，左右两侧改宽（贴着那一侧，往屏幕里侧长）。尺寸来自页面，不是正数就不理。
pub fn resize(app: &AppHandle, extent: f64) {
    if !(extent.is_finite() && extent > 0.0) {
        return;
    }
    let (r, sf, visible) = {
        let mut s = shown();
        s.extent = extent;
        (side_rect(s.work, s.side, extent), s.sf, s.how.is_some())
    };
    if visible {
        on_main(app, move |app| {
            if let Some(w) = app.get_webview_window(LABEL)
                && let Err(e) = apply(&w, r, sf)
            {
                log::warn!("晾衣绳改尺寸失败：{e}");
            }
        });
    }
}

/// 设置里改了位置（`configure` 之后调）：绳子放着时先收起，等页面收完，再在新位置放下亮一下（一直显示时在主屏常驻）；
/// 没放着什么都不做（一直显示时直接在新位置放下）。
pub fn relocate(app: &AppHandle) {
    if shown().how.is_none() {
        if pinned() {
            show_pinned(app);
        }
        return;
    }
    hide_now(app);
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("line-relocate".into())
        .spawn(move || {
            let t = Instant::now();
            while t.elapsed() < RELOCATE_WAIT {
                std::thread::sleep(TICK);
                if shown().how.is_none() {
                    return if pinned() {
                        show_pinned(&app)
                    } else {
                        show(&app, How::Flash)
                    };
                }
            }
            log::debug!("晾衣绳改位置后没等到收完，不再放下");
        });
    if let Err(e) = spawned {
        log::warn!("晾衣绳改位置线程启动失败：{e}");
    }
}

/// 晾衣绳窗口失去键盘（用户点了别的应用）：键盘已在用户点的地方，收起时不再切回原来的窗口。
pub fn blur() {
    let mut s = shown();
    s.keyboard = false;
    s.previous = None;
}

/// 页面要键盘（点到便签、进入编辑）或交还。交还在 Windows 上切回原来的窗口；
/// macOS 上平时键盘留到用户点别处或绳子收起（orderOut 时系统交还），一直显示时撤下再上屏，键盘交还前台应用，绳子留着。
pub fn keyboard(app: &AppHandle, on: bool) {
    if !on {
        let back = {
            let mut s = shown();
            s.keyboard = false;
            s.previous.take()
        };
        if let Some(h) = back {
            on_main(app, move |_| xpaste_platform::activate_window(h));
        }
        if cfg!(target_os = "macos") && pinned() {
            on_main(app, |app| {
                if let Some(w) = app.get_webview_window(LABEL)
                    && w.is_focused().unwrap_or(false)
                {
                    take_down(&w);
                    if let Err(e) = front(app, &w, false) {
                        log::warn!("显示晾衣绳失败：{e}");
                    }
                }
            });
        }
        return;
    }
    if shown().how.is_none() {
        return;
    }
    on_main(app, |app| {
        if let Some(w) = app.get_webview_window(LABEL)
            && let Err(e) = take_keyboard(app, &w)
        {
            log::warn!("晾衣绳取得焦点失败：{e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }
    fn cfg(side: LineSide) -> EdgeCfg {
        EdgeCfg { side, dock: None }
    }

    #[test]
    fn work_area_moves_below_notch_only_when_needed() {
        let r = |y, h| Rect {
            x: 0.0,
            y,
            w: 1512.0,
            h,
        };
        // 菜单栏常显：工作区已在菜单栏（比刘海高）下面，不动
        assert_eq!(below_notch(r(37.0, 945.0), 0.0, 32.0), r(37.0, 945.0));
        // 菜单栏自动隐藏：工作区从 0 开始，让到刘海下面
        assert_eq!(below_notch(r(0.0, 982.0), 0.0, 32.0), r(32.0, 950.0));
        // 没有刘海
        assert_eq!(below_notch(r(0.0, 900.0), 0.0, 0.0), r(0.0, 900.0));
        // 副屏在主屏上方（全局 y 为负）
        assert_eq!(
            below_notch(r(-1080.0, 1080.0), -1080.0, 0.0),
            r(-1080.0, 1080.0)
        );
    }

    #[test]
    fn hotkey_hangs_on_top_in_side_mode() {
        assert_eq!(hang_at(LineSide::Top), None);
        assert_eq!(hang_at(LineSide::Left), Some(0));
        assert_eq!(hang_at(LineSide::Right), Some(0));
    }

    #[test]
    fn duplicate_selects_only_when_line_is_down() {
        let dup = AppError::LineDuplicate(7);
        for how in [How::Flash, How::Mouse, How::Kbd, How::Drag] {
            assert_eq!(select_on_duplicate(Some(how), Some(&dup), true), Some(7));
            // 面板里挂的从不选中
            assert_eq!(select_on_duplicate(Some(how), Some(&dup), false), None);
        }
        assert_eq!(select_on_duplicate(None, Some(&dup), true), None);
        assert_eq!(select_on_duplicate(Some(How::Peek), Some(&dup), true), None);
        // 挂上了、或别的错误：照常亮一下
        assert_eq!(select_on_duplicate(Some(How::Flash), None, true), None);
        assert_eq!(
            select_on_duplicate(Some(How::Mouse), Some(&AppError::LineExpired), true),
            None
        );
    }

    #[test]
    fn drag_payload_is_flat_camel_case() {
        let item: DragItem = serde_json::from_value(serde_json::json!({
            "id": 3, "kind": "image", "title": "截图", "thumbUrl": "asset://t.png"
        }))
        .unwrap();
        let v = serde_json::to_value(DragHover {
            x: 1.0,
            y: 2.0,
            item,
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "x": 1.0, "y": 2.0, "id": 3, "kind": "image", "title": "截图", "thumbUrl": "asset://t.png"
            })
        );
    }

    #[test]
    fn toast_texts_and_extras() {
        let t = toast_for(None);
        assert_eq!(
            (t.text.as_str(), t.action, t.id),
            ("剪切板里还没有内容", None, None)
        );
        let t = toast_for(Some(&AppError::LineDuplicate(7)));
        assert_eq!(
            (t.text.as_str(), t.action, t.id),
            ("已经挂着了", None, Some(7))
        );
        let t = toast_for(Some(&AppError::LineExpired));
        assert_eq!(t.action, Some("pro"));
        assert!(t.text.starts_with("试用已结束"));
    }

    const MAIN: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1440.0,
        h: 900.0,
    };

    #[test]
    fn dwell_200ms_opens() {
        let mut e = Edge::default();
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 0, Press::Up, &cfg(LineSide::Top)),
            Some(EdgeEvent::Peek)
        );
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 190, Press::Up, &cfg(LineSide::Top)),
            None
        );
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 200, Press::Up, &cfg(LineSide::Top)),
            Some(EdgeEvent::Open(How::Mouse))
        );
        // 放下后停着不再发事件
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 400, Press::Up, &cfg(LineSide::Top)),
            None
        );
    }

    #[test]
    fn top_fling_still_needs_dwell() {
        // 快速推到菜单栏去点菜单：只露绳子，停够 200 ms 才放下
        let top = cfg(LineSide::Top);
        let mut e = Edge::default();
        assert_eq!(e.step((100.0, 300.0), &[MAIN], 0, Press::Up, &top), None);
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 16, Press::Up, &top),
            Some(EdgeEvent::Peek)
        );
        assert_eq!(e.step((100.0, 0.0), &[MAIN], 200, Press::Up, &top), None);
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 216, Press::Up, &top),
            Some(EdgeEvent::Open(How::Mouse))
        );
    }

    #[test]
    fn top_corners_do_not_count() {
        let top = cfg(LineSide::Top);
        assert!(!at_edge((20.0, 0.0), &[MAIN], &top)); // 左上角（苹果菜单、热角）
        assert!(!at_edge((1420.0, 0.0), &[MAIN], &top)); // 右上角（状态栏图标、热角）
        assert!(at_edge((40.0, 0.0), &[MAIN], &top));
        assert!(at_edge((1400.0, 0.0), &[MAIN], &top));
        // 每块屏各自的两角
        let right = Rect {
            x: 1440.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        assert!(!at_edge((1450.0, 0.0), &[MAIN, right], &top));
        let mut e = Edge::default();
        assert_eq!(e.step((20.0, 0.0), &[MAIN], 0, Press::Up, &top), None);
        assert_eq!(e.step((20.0, 0.0), &[MAIN], 300, Press::Up, &top), None);
    }

    #[test]
    fn click_during_dwell_cancels_until_leaving() {
        let top = cfg(LineSide::Top);
        let mut e = Edge::default();
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 0, Press::Up, &top),
            Some(EdgeEvent::Peek)
        );
        // 停着时点了菜单栏
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 100, Press::Held, &top),
            Some(EdgeEvent::Leave)
        );
        assert_eq!(e.step((100.0, 0.0), &[MAIN], 150, Press::Held, &top), None);
        // 松开后还在顶端，不再放下
        assert_eq!(e.step((100.0, 0.0), &[MAIN], 400, Press::Up, &top), None);
        assert_eq!(e.step((100.0, 0.0), &[MAIN], 700, Press::Up, &top), None);
        // 离开再回来照常
        assert_eq!(e.step((100.0, 5.0), &[MAIN], 720, Press::Up, &top), None);
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 740, Press::Up, &top),
            Some(EdgeEvent::Peek)
        );
    }

    #[test]
    fn held_without_drag_never_opens() {
        // 拖窗口贴顶、往上框选：按着键却不是在拖东西
        let top = cfg(LineSide::Top);
        let mut e = Edge::default();
        assert_eq!(e.step((100.0, 300.0), &[MAIN], 0, Press::Held, &top), None);
        assert_eq!(e.step((100.0, 0.0), &[MAIN], 16, Press::Held, &top), None);
        assert_eq!(e.step((100.0, 0.0), &[MAIN], 300, Press::Held, &top), None);
        // 松手时光标还在顶端
        assert_eq!(e.step((100.0, 0.0), &[MAIN], 320, Press::Up, &top), None);
        assert_eq!(e.step((100.0, 0.0), &[MAIN], 600, Press::Up, &top), None);
        // 左右两侧甩过去也一样
        let r = [rect(0., 0., 1440., 900.)];
        let left = cfg(LineSide::Left);
        let mut e = Edge::default();
        e.step((300., 450.), &r, 0, Press::Held, &left);
        assert_eq!(e.step((0., 450.), &r, 16, Press::Held, &left), None);
    }

    #[test]
    fn press_counts_as_drag_only_when_dragging() {
        // macOS：拖拽剪切板按下后变了才算
        assert_eq!(press_of(Some(5), Some(5), false), Press::Held);
        assert_eq!(press_of(Some(5), Some(6), false), Press::Drag);
        // Windows：没有计数，系统在拖动、缩放窗口时不算
        assert_eq!(press_of(None, None, false), Press::Drag);
        assert_eq!(press_of(None, None, true), Press::Held);
        assert_eq!(press_of(Some(5), Some(6), true), Press::Held);
    }

    #[test]
    fn press_while_peeking_cancels_even_without_drag_count() {
        // Windows：没有拖拽剪切板计数，按着一律像在拖；已经停在边缘时才按下的，也一定不是拖进来的
        let top = cfg(LineSide::Top);
        let (none, still) = (|| None, || false);
        let mut p = PressProbe::default();
        let mut e = Edge::default();
        let at = |e: &mut Edge, p: &mut PressProbe, button, t| {
            let press = p.sample(button, true, none, still);
            e.step((100.0, 0.0), &[MAIN], t, press, &top)
        };
        assert_eq!(at(&mut e, &mut p, false, 0), Some(EdgeEvent::Peek));
        assert_eq!(at(&mut e, &mut p, true, 100), Some(EdgeEvent::Leave));
        assert_eq!(at(&mut e, &mut p, true, 300), None);
        assert_eq!(at(&mut e, &mut p, false, 400), None);
        assert_eq!(at(&mut e, &mut p, false, 700), None);
        // 拖着东西进来照常放下
        let mut p = PressProbe::default();
        let mut e = Edge::default();
        assert_eq!(p.sample(true, false, none, still), Press::Held);
        assert_eq!(at(&mut e, &mut p, true, 0), Some(EdgeEvent::Peek));
        assert_eq!(
            at(&mut e, &mut p, true, 200),
            Some(EdgeEvent::Open(How::Drag))
        );
    }

    #[test]
    fn drag_baseline_is_taken_before_the_press() {
        use std::cell::Cell;
        let n = Cell::new(5);
        let count = || Some(n.get());
        let still = || false;
        let mut p = PressProbe::default();
        // 松着时记下 5
        assert_eq!(p.sample(false, false, count, still), Press::Up);
        // 按下就甩着拖：第一次采样前拖拽剪切板已经变了
        n.set(6);
        assert_eq!(p.sample(true, false, count, still), Press::Held); // 不在边缘不读
        assert_eq!(p.sample(true, true, count, still), Press::Drag);
        // 松开后按 6 重新记，下一次只是点一下
        assert_eq!(p.sample(false, false, count, still), Press::Up);
        assert_eq!(p.sample(true, true, count, still), Press::Held);
        // 刚醒来还没记过就按着：当在拖（同以前）
        let mut p = PressProbe::default();
        assert_eq!(p.sample(true, true, count, still), Press::Drag);
        // 系统在拖动窗口
        assert_eq!(p.sample(true, true, count, || true), Press::Held);
    }

    #[test]
    fn side_fling_still_opens_drag() {
        let r = [rect(0., 0., 1440., 900.)];
        let mut e = Edge::default();
        e.step((300., 450.), &r, 0, Press::Drag, &cfg(LineSide::Left));
        assert_eq!(
            e.step((0., 450.), &r, 16, Press::Drag, &cfg(LineSide::Left)),
            Some(EdgeEvent::Open(How::Drag))
        );
    }

    #[test]
    fn slow_arrival_needs_dwell() {
        let mut e = Edge::default();
        assert_eq!(
            e.step((100.0, 8.0), &[MAIN], 0, Press::Up, &cfg(LineSide::Top)),
            None
        );
        // 8 px / 16 ms = 0.5 px/ms
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 16, Press::Up, &cfg(LineSide::Top)),
            Some(EdgeEvent::Peek)
        );
    }

    #[test]
    fn leaving_peek_resets() {
        let mut e = Edge::default();
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 0, Press::Up, &cfg(LineSide::Top)),
            Some(EdgeEvent::Peek)
        );
        assert_eq!(
            e.step((100.0, 5.0), &[MAIN], 50, Press::Up, &cfg(LineSide::Top)),
            Some(EdgeEvent::Leave)
        );
        assert_eq!(
            e.step((100.0, 6.0), &[MAIN], 66, Press::Up, &cfg(LineSide::Top)),
            None
        );
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 100, Press::Up, &cfg(LineSide::Top)),
            Some(EdgeEvent::Peek)
        );
        // 从 100 重新计时
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 250, Press::Up, &cfg(LineSide::Top)),
            None
        );
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 300, Press::Up, &cfg(LineSide::Top)),
            Some(EdgeEvent::Open(How::Mouse))
        );
    }

    #[test]
    fn edge_ignores_shared_edge() {
        let below = Rect { y: 900.0, ..MAIN };
        let rects = [MAIN, below];
        assert!(!at_edge((100.0, 900.0), &rects, &cfg(LineSide::Top)));
        assert!(at_edge((100.0, 0.0), &rects, &cfg(LineSide::Top)));
        let mut e = Edge::default();
        assert_eq!(
            e.step((100.0, 900.0), &rects, 0, Press::Up, &cfg(LineSide::Top)),
            None
        );
        assert_eq!(
            e.step((100.0, 900.0), &rects, 300, Press::Up, &cfg(LineSide::Top)),
            None
        );
    }

    #[test]
    fn side_by_side_tops_both_count() {
        let right = Rect {
            x: 1440.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        let rects = [MAIN, right];
        assert!(at_edge((100.0, 0.0), &rects, &cfg(LineSide::Top)));
        assert!(at_edge((2000.0, 0.0), &rects, &cfg(LineSide::Top)));
        // 只有最顶端那一像素算，菜单栏其他位置不算
        assert!(at_edge((2000.0, 1.0), &rects, &cfg(LineSide::Top)));
        assert!(!at_edge((2000.0, 2.0), &rects, &cfg(LineSide::Top)));
    }

    #[test]
    fn button_down_opens_drag() {
        let mut e = Edge::default();
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 0, Press::Drag, &cfg(LineSide::Top)),
            Some(EdgeEvent::Peek)
        );
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 200, Press::Drag, &cfg(LineSide::Top)),
            Some(EdgeEvent::Open(How::Drag))
        );
    }

    #[test]
    fn pointer_inside_covers_menu_bar_strip() {
        // 工作区从菜单栏下面 25 起，窗口高 300
        let win = Rect {
            x: 0.0,
            y: 25.0,
            w: 1440.0,
            h: 300.0,
        };
        assert!(pointer_inside((100.0, 0.0), win, &[MAIN], LineSide::Top));
        assert!(pointer_inside((100.0, 324.0), win, &[MAIN], LineSide::Top));
        assert!(!pointer_inside((100.0, 325.0), win, &[MAIN], LineSide::Top));
        // 副屏在右边：窗口横向之外不算
        let right = Rect {
            x: 1440.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        assert!(!pointer_inside(
            (1500.0, 0.0),
            win,
            &[MAIN, right],
            LineSide::Top
        ));
        // 找不到所在屏时只算窗口本身
        assert!(!pointer_inside((100.0, 10.0), win, &[], LineSide::Top));
        assert!(pointer_inside((100.0, 30.0), win, &[], LineSide::Top));
    }

    #[test]
    fn off_screen_cursor_is_not_top() {
        assert!(!at_edge((100.0, 0.0), &[], &cfg(LineSide::Top)));
        assert!(!at_edge((-5.0, 0.0), &[MAIN], &cfg(LineSide::Top)));
    }

    #[test]
    fn side_edges_only_on_outer_borders() {
        let r = [rect(0., 0., 1440., 900.), rect(1440., 100., 1920., 1080.)];
        assert!(at_edge((0., 450.), &r, &cfg(LineSide::Left)));
        assert!(!at_edge((0., 20.), &r, &cfg(LineSide::Left))); // 上角 40 px 内
        assert!(!at_edge((0., 880.), &r, &cfg(LineSide::Left))); // 下角 40 px 内
        assert!(!at_edge((1439.5, 450.), &r, &cfg(LineSide::Right))); // A 的右边和 B 相接
        assert!(at_edge((1439.5, 50.), &r, &cfg(LineSide::Right))); // A 右边露在 B 上方的那段
        assert!(at_edge((3359.5, 600.), &r, &cfg(LineSide::Right)));
        assert!(!at_edge((1440., 600.), &r, &cfg(LineSide::Left))); // B 的左边和 A 相接
    }

    #[test]
    fn dock_on_same_side() {
        let r = [rect(0., 0., 1440., 900.)];
        let shown = EdgeCfg {
            side: LineSide::Left,
            dock: Some(Dock {
                rect: rect(0., 250., 70., 400.),
                side: DockSide::Left,
                autohide: false,
            }),
        };
        assert!(!at_edge((0., 450.), &r, &shown)); // 程序坞本身
        assert!(at_edge((0., 120.), &r, &shown)); // 程序坞上方露出的屏幕边
        let full = EdgeCfg {
            dock: Some(Dock {
                rect: rect(0., 0., 70., 900.),
                side: DockSide::Left,
                autohide: false,
            }),
            ..shown
        };
        assert!(!at_edge((0., 120.), &r, &full));
    }

    #[test]
    fn dock_on_other_side_is_ignored() {
        let r = [rect(0., 0., 1440., 900.)];
        let c = EdgeCfg {
            side: LineSide::Left,
            dock: Some(Dock {
                rect: rect(0., 250., 70., 400.),
                side: DockSide::Bottom,
                autohide: false,
            }),
        };
        assert!(at_edge((0., 450.), &r, &c));
    }

    #[test]
    fn dock_autohide_needs_longer_dwell_and_no_fling() {
        let r = [rect(0., 0., 1440., 900.)];
        let c = EdgeCfg {
            side: LineSide::Left,
            dock: Some(Dock {
                rect: rect(0., 250., 70., 400.),
                side: DockSide::Left,
                autohide: true,
            }),
        };
        let mut e = Edge::default();
        assert_eq!(e.step((200., 450.), &r, 0, Press::Up, &c), None);
        assert_eq!(
            e.step((0., 450.), &r, 16, Press::Up, &c),
            Some(EdgeEvent::Peek)
        ); // 甩过去也只露绳子
        assert_eq!(e.step((0., 450.), &r, 316, Press::Up, &c), None);
        assert_eq!(
            e.step((0., 450.), &r, 420, Press::Up, &c),
            Some(EdgeEvent::Open(How::Mouse))
        );
    }

    #[test]
    fn fling_follows_side_axis() {
        let r = [rect(0., 0., 1440., 900.)];
        let mut e = Edge::default();
        e.step((1300., 450.), &r, 0, Press::Up, &cfg(LineSide::Right));
        assert_eq!(
            e.step((1439.5, 450.), &r, 16, Press::Up, &cfg(LineSide::Right)),
            Some(EdgeEvent::Open(How::Mouse))
        );
    }

    #[test]
    fn dock_blocks_only_its_own_screen() {
        // B 在 A 左边、往下错开；程序坞在 A 的左边，占满 A 的整条边
        let r = [rect(0., 0., 1440., 900.), rect(-1920., 400., 1920., 1080.)];
        let c = EdgeCfg {
            side: LineSide::Left,
            dock: Some(Dock {
                rect: rect(0., 0., 70., 900.),
                side: DockSide::Left,
                autohide: false,
            }),
        };
        assert!(at_edge((-1920., 600.), &r, &c)); // B 的左边，和程序坞同高也算
        assert!(!at_edge((0., 200.), &r, &c)); // A 左边露在外面的那段被程序坞占着
    }

    #[test]
    fn side_rect_places_column() {
        let w = rect(0., 25., 1440., 875.);
        assert_eq!(
            side_rect(w, LineSide::Top, 240.),
            rect(0., 25., 1440., 240.)
        );
        assert_eq!(
            side_rect(w, LineSide::Left, 200.),
            rect(0., 25., 200., 875.)
        );
        assert_eq!(
            side_rect(w, LineSide::Right, 200.),
            rect(1240., 25., 200., 875.)
        );
        assert_eq!(
            side_rect(w, LineSide::Right, 5000.),
            rect(0., 25., 1440., 875.)
        );
    }

    #[test]
    fn dock_hints() {
        let s = rect(0., 0., 1440., 900.);
        let d = |y, h, autohide| Dock {
            rect: rect(0., y, 70., h),
            side: DockSide::Left,
            autohide,
        };
        assert_eq!(
            dock_hint(LineSide::Right, Some(&d(250., 400., false)), s),
            ""
        );
        // Windows 上叫任务栏
        let say = |mac: &'static str, win: &'static str| {
            if cfg!(target_os = "windows") {
                win
            } else {
                mac
            }
        };
        assert_eq!(
            dock_hint(LineSide::Left, Some(&d(250., 400., true)), s),
            say(
                "程序坞也在左侧，鼠标停得久一点才会放下绳子",
                "任务栏也在左侧，鼠标停得久一点才会放下绳子"
            )
        );
        assert_eq!(
            dock_hint(LineSide::Left, Some(&d(10., 880., false)), s),
            say(
                "程序坞占满了左侧，用快捷键放下晾衣绳",
                "任务栏占满了左侧，用快捷键放下晾衣绳"
            )
        );
        let r = |y, h, autohide| Dock {
            rect: rect(1370., y, 70., h),
            side: DockSide::Right,
            autohide,
        };
        assert_eq!(
            dock_hint(LineSide::Right, Some(&r(10., 880., false)), s),
            say(
                "程序坞占满了右侧，用快捷键放下晾衣绳",
                "任务栏占满了右侧，用快捷键放下晾衣绳"
            )
        );
        assert_eq!(
            dock_hint(LineSide::Left, Some(&d(250., 400., false)), s),
            ""
        );
    }

    #[test]
    fn pinned_hit_test_against_reported_rects() {
        let hits = [rect(100., 20., 152., 120.), rect(1380., 10., 28., 28.)];
        assert!(hit_test((100., 20.), &hits));
        assert!(hit_test((251.9, 139.9), &hits));
        // 四周放宽 4 px，再往外不算
        assert!(hit_test((96., 16.), &hits));
        assert!(hit_test((255.9, 60.), &hits));
        assert!(!hit_test((256., 60.), &hits));
        assert!(!hit_test((95.9, 60.), &hits));
        assert!(hit_test((1390., 30.), &hits));
        assert!(!hit_test((600., 60.), &hits)); // 绳子之间的空白
        assert!(!hit_test((100., 20.), &[]));
    }

    #[test]
    fn pinned_pass_through_holds_while_button_down() {
        // 没按键：落在能点的地方接收，其余穿透
        assert!(!pass_through(true, false, false, Some(true)));
        assert!(pass_through(false, false, false, Some(false)));
        // 按着键：保持按下时的状态，拖便签拖出卡片不断、在空白处按下拖过卡片也不截走
        assert!(!pass_through(false, true, false, Some(false)));
        assert!(pass_through(true, true, false, Some(true)));
        // 刚放下、还没定过：按光标算
        assert!(!pass_through(true, true, false, None));
        // 按着键碰到边缘（拖文件过来）：整窗接收
        assert!(!pass_through(false, true, true, Some(true)));
    }

    #[test]
    fn pinned_pass_through_survives_reshow_while_button_down() {
        let mut pin = Pin {
            pass: Some(false),
            generation: 1,
            ..Pin::default()
        };
        // 拖着便签时来了提醒（GEN 变了）：按着键先保持
        pin.regen(2, true);
        assert_eq!(pin.pass, Some(false));
        pin.regen(2, true);
        assert_eq!(pin.pass, Some(false));
        // 松开后按光标重新设
        pin.regen(2, false);
        assert_eq!(pin.pass, None);
        pin.pass = Some(true);
        pin.regen(2, false);
        assert_eq!(pin.pass, Some(true));
        // 没按键时 GEN 一变就重新设
        pin.regen(3, false);
        assert_eq!(pin.pass, None);
    }

    #[test]
    fn pinned_moves_only_to_another_screen() {
        let r = [rect(0., 0., 1440., 900.), rect(1440., 0., 1920., 1080.)];
        let win = rect(0., 25., 1440., 240.);
        assert!(!other_screen((100., 0.), win, &r));
        assert!(other_screen((2000., 0.), win, &r));
        // 窗口在副屏右侧
        let win = rect(3160., 0., 200., 1080.);
        assert!(other_screen((0., 450.), win, &r));
        assert!(!other_screen((3359., 450.), win, &r));
        // 光标不在任何屏上
        assert!(!other_screen((-50., 0.), win, &r));
    }

    #[test]
    fn pointer_inside_extends_to_side_edge() {
        let r = [rect(0., 0., 1440., 900.)];
        let win = rect(20., 25., 200., 875.);
        assert!(pointer_inside((5., 400.), win, &r, LineSide::Left));
        assert!(!pointer_inside((5., 400.), win, &r, LineSide::Top));
    }
}
