//! 晾衣绳浮层窗口 `line` 与屏幕顶端检测（只在带 Pro 的构建里启动）。
//! 窗口启动时预建、隐藏；macOS 是不激活应用的 NSPanel（同划词卡片），前台应用不变。
//! 检测线程每 16 ms 采样光标：碰到顶端先只露绳子（peek），停 200 ms 或甩上去才放下；
//! 放下后线程不再采样，收起由页面决定（离开、Esc、快捷键挂上后停够时间）。

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::events::{LINE_HIDE, LINE_SHOW, emit_to};
use crate::panel::{Rect, on_main};

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
    #[allow(dead_code)] // 便签快捷键接上后用到
    Flash,
    /// 键盘打开，拿键盘
    Kbd,
    /// 拖着东西到顶端
    Drag,
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

/// 绳子已放下（只露绳子不算）：检测线程据此停止采样
fn is_open() -> bool {
    shown().how.is_some_and(|h| h != How::Peek)
}

/// 预建窗口并起检测线程。只在带 Pro 的构建里调用。
pub fn start(app: &AppHandle) {
    prewarm(app);
    let app = app.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("line-edge".into())
        .spawn(move || detect(&app))
    {
        log::warn!("晾衣绳检测线程启动失败：{e}");
    }
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

/// 检测线程：每 16 ms 采样光标，屏幕矩形缓存 1 s；绳子放下期间不采样。
fn detect(app: &AppHandle) {
    let t0 = Instant::now();
    let mut edge = Edge::default();
    let mut screens = (Vec::new(), 1.0);
    let mut read_at: Option<Instant> = None;
    loop {
        std::thread::sleep(TICK);
        if is_open() {
            // 收起后先离开顶端才会再放下：按 Esc 收起时光标还在顶端，不立刻又放下来
            edge = Edge {
                last: None,
                phase: Phase::Open,
            };
            continue;
        }
        if read_at.is_none_or(|t| t.elapsed() >= SCREENS_TTL) {
            read_at = Some(Instant::now());
            match read_screens(app) {
                Ok(s) => screens = s,
                Err(e) => log::debug!("晾衣绳读取屏幕失败：{e}"),
            }
        }
        let Ok(c) = app.cursor_position() else {
            continue;
        };
        let (rects, psf) = (&screens.0, screens.1);
        let t = t0.elapsed().as_millis() as u64;
        // 拖文件、拖记录到顶端时主键按着
        let button = xpaste_platform::mouse_button_down();
        match edge.step((c.x / psf, c.y / psf), rects, t, button) {
            Some(EdgeEvent::Peek) => show(app, How::Peek),
            Some(EdgeEvent::Open(how)) => show(app, how),
            Some(EdgeEvent::Leave) => hide(app),
            None => {}
        }
    }
}

/// 全部屏幕的矩形与主屏缩放，光标除以主屏缩放后与之同一空间。换算同 `panel::monitor_contains`：
/// macOS 的光标按主屏缩放、显示器按各自缩放，都换回点；Windows 都是物理像素，统一除以主屏缩放，屏与屏相接的关系不变。
fn read_screens(app: &AppHandle) -> tauri::Result<(Vec<Rect>, f64)> {
    let psf = app.primary_monitor()?.map_or(1.0, |m| m.scale_factor());
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
            first
        };
        if first && let Err(e) = place(app, &w) {
            log::warn!("晾衣绳定位失败：{e}");
        }
        // 只露绳子时点击穿透到下面的应用
        if let Err(e) = w.set_ignore_cursor_events(how == How::Peek) {
            log::warn!("晾衣绳设置点击穿透失败：{e}");
        }
        emit_to(app, LABEL, LINE_SHOW, serde_json::json!({ "how": how }));
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
pub fn hide(app: &AppHandle) {
    if shown().how.is_some() {
        emit_to(app, LABEL, LINE_HIDE, ());
    }
}

/// 页面播完收起：撤下窗口、恢复点击穿透，拿过键盘时交还；之后检测线程继续采样。
pub fn hidden(app: &AppHandle) {
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
    fn off_screen_cursor_is_not_top() {
        assert!(!at_top_edge((100.0, 0.0), &[]));
        assert!(!at_top_edge((-5.0, 0.0), &[MAIN]));
    }
}
