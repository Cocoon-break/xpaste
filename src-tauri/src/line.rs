//! 晾衣绳浮层窗口 `line` 与屏幕顶端检测（只在带 Pro 的构建里启动）。
//! 窗口启动时预建、隐藏；macOS 是不激活应用的 NSPanel（同划词卡片），前台应用不变。
//! 检测线程每 16 ms 采样光标：碰到顶端先只露绳子（peek），停 200 ms 或甩上去才放下；
//! 放下后线程接着采样，只把光标进出便签区域（`line-pointer`）告诉页面，收起由页面决定（离开、Esc、快捷键挂上后停够时间）。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

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

/// 停在顶端这么久才放下
const DWELL_MS: u64 = 200;
/// 往上甩的速度（px/ms）达到它，到顶端就直接放下
const FLING: f64 = 0.85;
const TICK: Duration = Duration::from_millis(16);
/// 屏幕矩形缓存这么久
const SCREENS_TTL: Duration = Duration::from_secs(1);
/// 窗口初始高度（逻辑像素）；页面渲染后经 `line_resize` 改
const H0: f64 = 240.0;
/// 发出 `line-hide` 后页面这么久还没调 `line_hidden`（页面卡住、崩了），后端自己收起
const HIDDEN_FALLBACK: Duration = Duration::from_millis(1000);

#[derive(Clone, Copy)]
enum Phase {
    Away,
    /// 到顶端的时刻
    Peek(u64),
    /// 已放下：离开顶端前不再发事件
    Open,
}

/// 顶端检测的状态机，见 `step`。
pub struct Edge {
    /// 上一次采样 (y, t)
    last: Option<(f64, u64)>,
    phase: Phase,
}

impl Default for Edge {
    fn default() -> Self {
        Self {
            last: None,
            phase: Phase::Away,
        }
    }
}

impl Edge {
    /// 每 16 ms 采样一次：碰到顶端停 200 ms 或甩上去才放下，两屏相接的边不算顶端。
    /// rects：全部屏幕的逻辑坐标矩形；button：鼠标键按着（拖文件时为 Drag）
    pub fn step(
        &mut self,
        c: (f64, f64),
        rects: &[Rect],
        t_ms: u64,
        button: bool,
    ) -> Option<EdgeEvent> {
        let top = at_top_edge(c, rects);
        let fling = self
            .last
            .is_some_and(|(y, t)| t_ms > t && (y - c.1) / (t_ms - t) as f64 >= FLING);
        self.last = Some((c.1, t_ms));
        let how = if button { How::Drag } else { How::Mouse };
        match (self.phase, top) {
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
            (Phase::Away, true) if !fling => {
                self.phase = Phase::Peek(t_ms);
                Some(EdgeEvent::Peek)
            }
            (Phase::Peek(since), true) if !fling && t_ms - since < DWELL_MS => None,
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

/// 光标在某块屏最顶端那一像素，且上方没有别的屏（两屏上下相接的边不算顶端）。
pub fn at_top_edge(c: (f64, f64), rects: &[Rect]) -> bool {
    rects
        .iter()
        .find(|r| contains(r, c))
        .is_some_and(|r| c.1 - r.y <= 1.0 && !rects.iter().any(|o| contains(o, (c.0, r.y - 1.0))))
}

/// 光标在便签区域里：晾衣绳窗口的矩形往上延到所在屏的顶端（菜单栏那一条也算，光标停在顶端时就在那里）。
/// `win` 与 `rects`、`c` 同一坐标空间。
pub fn pointer_inside(c: (f64, f64), win: Rect, rects: &[Rect]) -> bool {
    let top = rects
        .iter()
        .find(|r| contains(r, (win.x, win.y)))
        .map_or(win.y, |r| r.y);
    c.0 >= win.x && c.0 < win.x + win.w && c.1 >= top && c.1 < win.y + win.h
}

/// 窗口这一侧的状态。`how` 为 None：窗口已隐藏（页面调过 `line_hidden`）。
struct Shown {
    how: Option<How>,
    /// 所在屏的工作区（逻辑像素）与缩放，放下时记下
    work: Rect,
    sf: f64,
    h: f64,
    keyboard: bool,
    /// Windows：拿键盘前的前台窗口，交还键盘时切回去（macOS 恒为 None，orderOut 时系统自己交还）
    previous: Option<isize>,
}

/// 这个构建带晾衣绳，且是官方签名的那份（被改过再重签的副本表现同开源版）：窗口、顶端检测、快捷键都看它
pub fn enabled() -> bool {
    xpaste_pro::AVAILABLE && xpaste_pro::genuine()
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
    h: H0,
    keyboard: false,
    previous: None,
});

fn shown() -> MutexGuard<'static, Shown> {
    SHOWN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 页面已开始监听 `line-show`；之前检测线程只睡，不检测顶端（页面没加载好时放下的窗口会挡住顶端的点击）
static READY: AtomicBool = AtomicBool::new(false);

/// 页面开始监听 line-show 后调用；之前不检测顶端
pub fn ready() {
    READY.store(true, Ordering::Relaxed);
    log::info!("晾衣绳页面就绪，开始检测顶端");
    // 关着应用时错过的提醒现在补发
    crate::remind::wake();
}

/// 页面已就绪：之前提醒线程不放下绳子
pub fn is_ready() -> bool {
    READY.load(Ordering::Relaxed)
}

/// 绳子已放下（只露绳子不算）时窗口的矩形，换到检测线程的坐标空间（除以 `k` 之后）；没放下为 None。
/// macOS 两边都是点；Windows 窗口按所在屏缩放记的逻辑像素，检测线程按主屏缩放，差一个 sf / k。
fn open_rect(k: f64) -> Option<Rect> {
    let s = shown();
    s.how.filter(|&h| h != How::Peek)?;
    let f = if cfg!(target_os = "macos") {
        1.0
    } else {
        s.sf / k
    };
    let w = s.work;
    Some(Rect {
        x: w.x * f,
        y: w.y * f,
        w: w.w * f,
        h: s.h.min(w.h) * f,
    })
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
        Ok(Some(id)) => crate::commands::line::hang_record(app, &state, id, None)
            .map(|_| ())
            .map_err(Some),
        Ok(None) => Err(None),
        Err(e) => {
            log::warn!("读取最新记录失败：{e}");
            Err(Some(e.into()))
        }
    };
    let err = res.err();
    let how = shown().how;
    if let Some(sel) = select_on_duplicate(how, err.as_ref().and_then(Option::as_ref), select) {
        return show_selecting(app, How::Kbd, Some(sel));
    }
    show(app, How::Flash);
    if let Some(e) = err {
        emit_to(app, LABEL, LINE_TOAST, toast_for(e.as_ref()));
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

/// 检测线程：每 16 ms 采样光标，屏幕矩形缓存 1 s；绳子放下期间只报光标进出便签区域。
fn detect(app: &AppHandle) {
    let t0 = Instant::now();
    let mut edge = Edge::default();
    let mut screens = (Vec::new(), 1.0);
    let mut read_at: Option<Instant> = None;
    // 上次报给页面的进出；每次放下后先报一次
    let mut inside: Option<bool> = None;
    loop {
        std::thread::sleep(TICK);
        if !READY.load(Ordering::Relaxed) {
            continue;
        }
        if read_at.is_none_or(|t| t.elapsed() >= SCREENS_TTL) {
            read_at = Some(Instant::now());
            match read_screens(app) {
                Ok(s) => screens = s,
                Err(e) => log::debug!("晾衣绳读取屏幕失败：{e}"),
            }
        }
        // 不用 app.cursor_position()：它要等主线程应答，每秒 60 次会一直唤醒主线程
        let Some((x, y)) = xpaste_platform::pointer_position() else {
            continue;
        };
        let (rects, k) = (&screens.0, screens.1);
        let c = (x / k, y / k);
        if let Some(win) = open_rect(k) {
            // 收起后先离开顶端才会再放下：按 Esc 收起时光标还在顶端，不立刻又放下来
            edge = Edge {
                last: None,
                phase: Phase::Open,
            };
            let now_in = pointer_inside(c, win, rects);
            if inside != Some(now_in) {
                inside = Some(now_in);
                emit_to(
                    app,
                    LABEL,
                    LINE_POINTER,
                    serde_json::json!({ "inside": now_in }),
                );
            }
            continue;
        }
        inside = None;
        let t = t0.elapsed().as_millis() as u64;
        // 拖文件、拖记录到顶端时主键按着
        let button = xpaste_platform::mouse_button_down();
        match edge.step(c, rects, t, button) {
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

/// 放到光标所在屏工作区顶端，宽为工作区宽，发 `line-show { how }`。
/// 已放下时不退回只露绳子；只露绳子之后再放下只换 `how`，窗口不挪。`Kbd` 拿键盘，其余不拿。
pub fn show(app: &AppHandle, how: How) {
    show_selecting(app, how, None);
}

/// 同 `show`，`line-show` 另带要选中的便签 `id`（键盘落在它上面）。
fn show_selecting(app: &AppHandle, how: How, select: Option<u64>) {
    log::info!("晾衣绳放下 {how:?}");
    on_main(app, move |app| {
        let Some(w) = app.get_webview_window(LABEL) else {
            return log::warn!("找不到晾衣绳窗口");
        };
        let first = {
            let mut s = shown();
            if how == How::Peek && s.how.is_some_and(|h| h != How::Peek) {
                return;
            }
            let first = s.how.is_none();
            s.how = Some(how);
            GEN.fetch_add(1, Ordering::Relaxed);
            first
        };
        if first && let Err(e) = place(app, &w) {
            log::warn!("晾衣绳定位失败：{e}");
        }
        // 只露绳子时点击穿透到下面的应用
        if let Err(e) = w.set_ignore_cursor_events(how == How::Peek) {
            log::warn!("晾衣绳设置点击穿透失败：{e}");
        }
        emit_to(
            app,
            LABEL,
            LINE_SHOW,
            serde_json::json!({ "how": how, "id": select }),
        );
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

/// 光标所在屏的工作区（避开菜单栏、程序坞、任务栏）顶端。主线程调用。
fn place(app: &AppHandle, w: &WebviewWindow) -> tauri::Result<()> {
    let Some(m) = crate::panel::cursor_monitor(app)? else {
        return Ok(());
    };
    let (sf, wa) = (m.scale_factor(), m.work_area());
    let work = Rect {
        x: wa.position.x as f64 / sf,
        y: wa.position.y as f64 / sf,
        w: wa.size.width as f64 / sf,
        h: wa.size.height as f64 / sf,
    };
    let h = {
        let mut s = shown();
        s.work = work;
        s.sf = sf;
        s.h
    };
    apply(w, work, sf, h)
}

/// 摆窗口：macOS 用逻辑坐标（点），Windows 按所在屏缩放换成物理像素（同划词卡片）。高度不超过工作区。
fn apply(w: &WebviewWindow, work: Rect, sf: f64, h: f64) -> tauri::Result<()> {
    let k = if cfg!(target_os = "macos") { 1.0 } else { sf };
    crate::shot::place(w, (work.x * k, work.y * k), (work.w * k, h.min(work.h) * k))
}

/// 发 `line-hide`；页面收起后调 `line_hidden`，窗口才真正隐藏。
/// 1 s 内页面没调 `line_hidden`、其间也没再放下，就自己收起，免得页面卡住时窗口一直挡在顶端。
pub fn hide(app: &AppHandle) {
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
        // NSPanel 上 hide 即 orderOut，键盘交还仍处于激活状态的前台应用
        if let Err(e) = w.hide() {
            log::warn!("隐藏晾衣绳失败：{e}");
        }
        // Windows：经 show_no_activate 上屏时 tao 的标记仍是隐藏，上面的 hide() 不会真的隐藏
        #[cfg(windows)]
        match w.hwnd() {
            Ok(h) => xpaste_platform::hide_window(h.0 as isize),
            Err(e) => log::warn!("晾衣绳取不到窗口句柄：{e}"),
        }
    });
}

/// 页面按内容改高度；宽和位置不变。高度来自页面，不是正数就不理。
pub fn resize(app: &AppHandle, h: f64) {
    if !(h.is_finite() && h > 0.0) {
        return;
    }
    let (work, sf, visible) = {
        let mut s = shown();
        s.h = h;
        (s.work, s.sf, s.how.is_some())
    };
    if visible {
        on_main(app, move |app| {
            if let Some(w) = app.get_webview_window(LABEL)
                && let Err(e) = apply(&w, work, sf, h)
            {
                log::warn!("晾衣绳改高度失败：{e}");
            }
        });
    }
}

/// 晾衣绳窗口失去键盘（用户点了别的应用）：键盘已在用户点的地方，收起时不再切回原来的窗口。
pub fn blur() {
    let mut s = shown();
    s.keyboard = false;
    s.previous = None;
}

/// 页面要键盘（点到便签、进入编辑）或交还。交还只在 Windows 上切回原来的窗口；
/// macOS 上键盘留到用户点别处或绳子收起（orderOut 时系统交还）。
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
            e.step((100.0, 0.0), &[MAIN], 0, false),
            Some(EdgeEvent::Peek)
        );
        assert_eq!(e.step((100.0, 0.0), &[MAIN], 190, false), None);
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 200, false),
            Some(EdgeEvent::Open(How::Mouse))
        );
        // 放下后停着不再发事件
        assert_eq!(e.step((100.0, 0.0), &[MAIN], 400, false), None);
    }

    #[test]
    fn fling_opens_immediately() {
        let mut e = Edge::default();
        assert_eq!(e.step((100.0, 300.0), &[MAIN], 0, false), None);
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 16, false),
            Some(EdgeEvent::Open(How::Mouse))
        );
    }

    #[test]
    fn slow_arrival_needs_dwell() {
        let mut e = Edge::default();
        assert_eq!(e.step((100.0, 8.0), &[MAIN], 0, false), None);
        // 8 px / 16 ms = 0.5 px/ms
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 16, false),
            Some(EdgeEvent::Peek)
        );
    }

    #[test]
    fn leaving_peek_resets() {
        let mut e = Edge::default();
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 0, false),
            Some(EdgeEvent::Peek)
        );
        assert_eq!(
            e.step((100.0, 5.0), &[MAIN], 50, false),
            Some(EdgeEvent::Leave)
        );
        assert_eq!(e.step((100.0, 6.0), &[MAIN], 66, false), None);
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 100, false),
            Some(EdgeEvent::Peek)
        );
        // 从 100 重新计时
        assert_eq!(e.step((100.0, 0.0), &[MAIN], 250, false), None);
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 300, false),
            Some(EdgeEvent::Open(How::Mouse))
        );
    }

    #[test]
    fn edge_ignores_shared_edge() {
        let below = Rect { y: 900.0, ..MAIN };
        let rects = [MAIN, below];
        assert!(!at_top_edge((100.0, 900.0), &rects));
        assert!(at_top_edge((100.0, 0.0), &rects));
        let mut e = Edge::default();
        assert_eq!(e.step((100.0, 900.0), &rects, 0, false), None);
        assert_eq!(e.step((100.0, 900.0), &rects, 300, false), None);
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
        assert!(at_top_edge((100.0, 0.0), &rects));
        assert!(at_top_edge((2000.0, 0.0), &rects));
        // 只有最顶端那一像素算，菜单栏其他位置不算
        assert!(at_top_edge((2000.0, 1.0), &rects));
        assert!(!at_top_edge((2000.0, 2.0), &rects));
    }

    #[test]
    fn button_down_opens_drag() {
        let mut e = Edge::default();
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 0, true),
            Some(EdgeEvent::Peek)
        );
        assert_eq!(
            e.step((100.0, 0.0), &[MAIN], 200, true),
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
        assert!(pointer_inside((100.0, 0.0), win, &[MAIN]));
        assert!(pointer_inside((100.0, 324.0), win, &[MAIN]));
        assert!(!pointer_inside((100.0, 325.0), win, &[MAIN]));
        // 副屏在右边：窗口横向之外不算
        let right = Rect {
            x: 1440.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        assert!(!pointer_inside((1500.0, 0.0), win, &[MAIN, right]));
        // 找不到所在屏时只算窗口本身
        assert!(!pointer_inside((100.0, 10.0), win, &[]));
        assert!(pointer_inside((100.0, 30.0), win, &[]));
    }

    #[test]
    fn off_screen_cursor_is_not_top() {
        assert!(!at_top_edge((100.0, 0.0), &[]));
        assert!(!at_top_edge((-5.0, 0.0), &[MAIN]));
    }
}
