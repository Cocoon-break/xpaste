//! 传送带窗口：贴在光标所在屏工作区的底边、水平居中，不拿键盘，透明处点击穿透。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::belt::{HITS, LABEL};
use crate::events::{BELT_SHOWN, emit_to};
use crate::float_drag::{self, Side};
use crate::line::{hit_test, pass_through};
use crate::panel::{Rect, on_main};
use crate::state::AppState;

pub const PAGE: &str = "pro/web/belt.html";

/// 带身高度
const BODY_H: f64 = 64.0;
/// 窗口比带身高出的部分，放提示条、探出提示和“下一件”标签；这一块透明处点击穿透
pub const TOP_ROOM: f64 = 96.0;
/// 带身底边离工作区底边
const BOTTOM_GAP: f64 = 16.0;
const TICK: Duration = Duration::from_millis(16);
/// 要收起以后等页面放手（`belt_release`）最多这么久，过了就自己藏
const RELEASE_SAFETY: Duration = Duration::from_secs(5);

/// 带子最短长度（逻辑像素）
pub const MIN_LEN: f64 = 360.0;
/// 记下的带子长度（逻辑像素，整数字符串）
const LEN_KEY: &str = "belt_len";

/// 窗口矩形（逻辑像素）：工作区里水平居中，宽 min(800, 工作区宽 × 0.84)，带身底边在工作区底边上方 16 px。
pub fn belt_rect(work: Rect) -> Rect {
    let w = (work.w * 0.84).min(800.0);
    let h = BODY_H + TOP_ROOM;
    Rect {
        x: work.x + (work.w - w) / 2.0,
        y: work.y + work.h - BOTTOM_GAP - h,
        w,
        h,
    }
}

/// 没拖过位置时的窗口矩形：右边界让到搅拌机本身左边再留 16 px（`mixer_left` 为 None 时到工作区右边），
/// 在剩下的宽里居中；调过的长度 `len` 是上限，让着搅拌机放不下时临时缩到可用宽 − 32（不小于 `MIN_LEN`）。
/// 搅拌机左边放不下最短的带子（拖到了左半边、在别的屏上）时不让。
pub fn belt_room(work: Rect, mixer_left: Option<f64>, len: Option<f64>) -> Rect {
    let right = mixer_left
        .map(|l| l - 16.0)
        .filter(|r| *r <= work.x + work.w && *r - work.x - 32.0 >= MIN_LEN);
    let avail = right.map_or(work.w, |r| r - work.x);
    let mut d = belt_rect(Rect { w: avail, ..work });
    if let Some(len) = len {
        let cap = if right.is_some() { avail - 32.0 } else { avail };
        d.w = len.min(cap).max(MIN_LEN);
        d.x = work.x + (avail - d.w) / 2.0;
    }
    d
}

/// 带子窗口正在显示（打开或探出），指针线程只在这期间跑
static SHOWN: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicBool = AtomicBool::new(false);
/// 每次显示加一：指针线程发现变了就忘掉上次设下的点击穿透，按光标重新设
static GEN: AtomicU64 = AtomicU64::new(0);
/// 每次要收起加一：超时藏窗口前看一眼，之后又要收起过的话这次作废
static LINGER: AtomicU64 = AtomicU64::new(0);
/// 窗口左上角和换成逻辑像素要除的系数，都在 `pointer_position` 的坐标空间里
/// （macOS 是点，系数 1；Windows 是物理像素，系数为所在屏缩放）
static FRAME: float_drag::Frame = Mutex::new(((0.0, 0.0), 1.0));
/// 正在拖动换位置和这次窗口的逻辑大小（宽随屏）
static DRAG: float_drag::Dragging = Mutex::new(None);
/// 正在拖一端调长度：按下时的状态，`last` 为上次投出去的光标横坐标
struct Resize {
    side: Side,
    cursor_x: f64,
    origin: (f64, f64),
    w0: f64,
    centered: bool,
    /// 最长：居中的让着搅拌机（`belt_room`），拖过位置的到工作区宽
    max: f64,
    /// 按下时光标所在屏的工作区（逻辑像素）和换算系数：拖过屏也按这块算
    work: Rect,
    k: f64,
    last: Option<f64>,
}
static RESIZE: Mutex<Option<Resize>> = Mutex::new(None);
static SIZE: Mutex<(f64, f64)> = Mutex::new((800.0, BODY_H + TOP_ROOM));
/// 这次显示所在屏的工作区（逻辑像素）和换算系数：重新摆时按这块算，不跟着光标换屏
static WORK: Mutex<(Rect, f64)> = Mutex::new((
    Rect {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    },
    1.0,
));
/// 每次重新摆加一：滑到一半又要重新摆、显示、藏起、开始拖动或调长度时旧的那次停下
static SLIDE: AtomicU64 = AtomicU64::new(0);

fn stop_slide() {
    SLIDE.fetch_add(1, Ordering::SeqCst);
}
const SLIDE_STEPS: u32 = 12;

/// 建好隐藏的窗口，免得第一次显示时现建 webview。参数同晾衣绳。
pub fn prewarm(app: &AppHandle) {
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(PAGE.into()))
        .title("xpaste-belt")
        .theme(crate::panel::current_theme(app))
        .visible(false)
        .focused(false)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .resizable(false)
        .minimizable(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .accept_first_mouse(true)
        .inner_size(800.0, BODY_H + TOP_ROOM)
        .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
        .build();
    match built {
        #[cfg(target_os = "macos")]
        Ok(w) => on_main(app, move |_| {
            // 同晾衣绳的层级，比面板低一层；全屏空间里也显示（to_overlay_panel 已带上）
            if let Err(e) = crate::shot::to_overlay_panel(&w, 2) {
                log::warn!("传送带窗口转换 NSPanel 失败：{e}");
            }
        }),
        #[cfg(windows)]
        Ok(w) => on_main(app, move |_| match w.hwnd() {
            Ok(h) => xpaste_platform::tool_window(h.0 as isize),
            Err(e) => log::warn!("传送带取不到窗口句柄：{e}"),
        }),
        #[cfg(not(any(target_os = "macos", windows)))]
        Ok(_) => {}
        Err(e) => log::warn!("创建传送带窗口失败：{e}"),
    }
}

/// 摆到光标所在屏：拖过的照记下的位置和长度，没拖过放底边、让开搅拌机居中（`belt_room`）。主线程调用。
fn place(app: &AppHandle, w: &WebviewWindow) -> tauri::Result<()> {
    let Some((work, k)) = float_drag::cursor_work(app)? else {
        return Ok(());
    };
    *WORK.lock().unwrap_or_else(PoisonError::into_inner) = (work, k);
    let offset = float_drag::saved(app, LABEL);
    let mixer = offset
        .is_none()
        .then(crate::mixer_window::body_left)
        .flatten();
    let r = float_drag::placed(work, belt_room(work, mixer, saved_len(app)), offset);
    set_rect(w, r, k)
}

/// 窗口摆到 `r`（逻辑像素）并更新 `FRAME`、`SIZE`。主线程调用。
fn set_rect(w: &WebviewWindow, r: Rect, k: f64) -> tauri::Result<()> {
    *FRAME.lock().unwrap_or_else(PoisonError::into_inner) = ((r.x * k, r.y * k), k);
    *SIZE.lock().unwrap_or_else(PoisonError::into_inner) = (r.w, r.h);
    crate::shot::place(w, (r.x * k, r.y * k), (r.w * k, r.h * k))
}

/// 拖着换位置或调长度
fn busy() -> bool {
    DRAG.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_some()
        || RESIZE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
}

/// 搅拌机打开、关掉、收成角标、展开之后：带子在屏上、没拖过位置时按 `belt_room` 重新摆，
/// 12 步、每步 16 ms 滑过去（减少动态效果时一步到位）；拖过的、正拖着的不动。
pub fn relayout(app: &AppHandle) {
    on_main(app, |app| {
        if !shown() || busy() || float_drag::saved(app, LABEL).is_some() {
            return;
        }
        let (work, k) = *WORK.lock().unwrap_or_else(PoisonError::into_inner);
        // 这次显示时没摆成（没取到光标所在屏）
        if work.w <= 0.0 {
            return;
        }
        let to = belt_room(work, crate::mixer_window::body_left(), saved_len(app));
        let ((ox, oy), _) = *FRAME.lock().unwrap_or_else(PoisonError::into_inner);
        let (fw, fh) = *SIZE.lock().unwrap_or_else(PoisonError::into_inner);
        let from = Rect {
            x: ox / k,
            y: oy / k,
            w: fw,
            h: fh,
        };
        if from == to {
            return;
        }
        let steps = if crate::panel::reduce_motion() {
            1
        } else {
            SLIDE_STEPS
        };
        let g = SLIDE.fetch_add(1, Ordering::SeqCst) + 1;
        let a = app.clone();
        let spawned = std::thread::Builder::new()
            .name("belt-slide".into())
            .spawn(move || {
                for i in 1..=steps {
                    if i > 1 {
                        std::thread::sleep(TICK);
                    }
                    if SLIDE.load(Ordering::SeqCst) != g {
                        return;
                    }
                    // 先快后慢
                    let t = 1.0 - (1.0 - f64::from(i) / f64::from(steps)).powi(3);
                    let lerp = |p: f64, q: f64| p + (q - p) * t;
                    let r = Rect {
                        x: lerp(from.x, to.x),
                        y: lerp(from.y, to.y),
                        w: lerp(from.w, to.w),
                        h: lerp(from.h, to.h),
                    };
                    on_main(&a, move |app| {
                        if SLIDE.load(Ordering::SeqCst) != g {
                            return;
                        }
                        if !shown() || busy() {
                            stop_slide();
                            return;
                        }
                        if let Some(w) = app.get_webview_window(LABEL)
                            && let Err(e) = set_rect(&w, r, k)
                        {
                            log::warn!("传送带让开搅拌机失败：{e}");
                        }
                    });
                }
            });
        if let Err(e) = spawned {
            log::warn!("传送带重新摆的线程启动失败：{e}");
        }
    });
}

fn saved_len(app: &AppHandle) -> Option<f64> {
    let v = app.state::<AppState>().store().kv_get(LEN_KEY).ok()??;
    v.parse().ok()
}

/// 页面在某一端的边上按下：开始拖动调长度
pub fn resize(app: &AppHandle, side: Side) {
    // 只在带子打开着时；拖动换位置中不开始
    if !shown()
        || crate::belt::open_stamp().is_none()
        || DRAG
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    {
        return;
    }
    stop_slide();
    let (tx, rx) = std::sync::mpsc::channel();
    on_main(app, move |app| {
        let _ = tx.send(float_drag::cursor_work(app).ok().flatten());
    });
    let Ok(Some((work, k))) = rx.recv_timeout(Duration::from_secs(1)) else {
        return;
    };
    let Some(cursor) = xpaste_platform::pointer_position() else {
        return;
    };
    let (origin, _) = *FRAME.lock().unwrap_or_else(PoisonError::into_inner);
    let w0 = SIZE.lock().unwrap_or_else(PoisonError::into_inner).0;
    let centered = float_drag::saved(app, LABEL).is_none();
    let max = if centered {
        belt_room(work, crate::mixer_window::body_left(), Some(work.w)).w
    } else {
        work.w
    };
    *RESIZE.lock().unwrap_or_else(PoisonError::into_inner) = Some(Resize {
        side,
        cursor_x: cursor.0,
        origin,
        w0,
        centered,
        max,
        work,
        k,
        last: None,
    });
}

/// 指针线程每次采样调用：按着键时把新宽度投到主线程；松开时结束并记下长度
fn resize_tick(app: &AppHandle, cursor_x: f64, button: bool) {
    let mut g = RESIZE.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(r) = g.as_mut() else {
        return;
    };
    if !button {
        let (moved, centered, work) = (r.last.is_some(), r.centered, r.work);
        *g = None;
        drop(g);
        if moved {
            on_main(app, move |app| finish_resize(app, centered, work));
        }
        return;
    }
    if r.last == Some(cursor_x) {
        return;
    }
    r.last = Some(cursor_x);
    let st = (
        r.side,
        cursor_x - r.cursor_x,
        r.origin,
        r.w0,
        r.centered,
        r.max,
    );
    let (work, k) = (r.work, r.k);
    drop(g);
    on_main(app, move |app| apply_resize(app, st, work, k));
}

/// 按光标位移算新矩形，限在按下时所在屏的工作区里并更新 `FRAME`、`SIZE`。主线程调用。
fn apply_resize(
    app: &AppHandle,
    (side, dx_px, origin, w0, centered, max): (Side, f64, (f64, f64), f64, bool, f64),
    work: Rect,
    k: f64,
) {
    let Some(w) = app.get_webview_window(LABEL) else {
        return;
    };
    let h = SIZE.lock().unwrap_or_else(PoisonError::into_inner).1;
    let (o, nw) = float_drag::resized(
        (origin.0 / k, origin.1 / k),
        w0,
        dx_px / k,
        side,
        centered,
        MIN_LEN.min(work.w),
        max,
    );
    let o = float_drag::clamp(o, (nw, h), work);
    *FRAME.lock().unwrap_or_else(PoisonError::into_inner) = ((o.0 * k, o.1 * k), k);
    *SIZE.lock().unwrap_or_else(PoisonError::into_inner) = (nw, h);
    if let Err(e) = crate::shot::place(&w, (o.0 * k, o.1 * k), (nw * k, h * k)) {
        log::warn!("传送带调长度失败：{e}");
    }
}

/// 松手：记下长度；位置拖过的话把角也重新记下（居中的保持自动居中）。主线程调用。
fn finish_resize(app: &AppHandle, centered: bool, work: Rect) {
    let len = SIZE.lock().unwrap_or_else(PoisonError::into_inner).0;
    if let Err(e) = app
        .state::<AppState>()
        .store()
        .kv_set(LEN_KEY, &format!("{len:.0}"))
    {
        log::warn!("传送带记下长度失败：{e}");
    }
    if centered {
        // 调的时候搅拌机开了、关了：松手后补上这次没做的让开
        relayout(app);
    } else {
        float_drag::save_in(app, LABEL, &FRAME, work);
    }
}

/// 页面在左端把手上按下：开始拖动换位置
pub fn drag() {
    if shown()
        && RESIZE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none()
    {
        stop_slide();
        let size = *SIZE.lock().unwrap_or_else(PoisonError::into_inner);
        float_drag::start(&DRAG, &FRAME, size);
    }
}

/// 显示带子，不拿键盘。已经显示着就不动，关闭之前不跟着光标换屏。
pub fn show(app: &AppHandle) {
    on_main(app, |app| {
        let Some(w) = app.get_webview_window(LABEL) else {
            return;
        };
        if SHOWN.swap(true, Ordering::SeqCst) {
            return;
        }
        stop_slide();
        let shown = place(app, &w)
            .and_then(|()| w.set_ignore_cursor_events(true))
            .and_then(|()| crate::trcard::front(app, &w, false));
        if let Err(e) = shown {
            log::warn!("显示传送带失败：{e}");
            SHOWN.store(false, Ordering::SeqCst);
            return;
        }
        GEN.fetch_add(1, Ordering::SeqCst);
        if !RUNNING.swap(true, Ordering::SeqCst) {
            spawn_pointer(app);
        }
        // 藏起时清掉了点击区域，让页面重报
        emit_to(app, LABEL, BELT_SHOWN, ());
    });
}

/// 带子窗口在屏上（含收起后等页面放手的那一会儿）
pub fn shown() -> bool {
    SHOWN.load(Ordering::SeqCst)
}

/// 带子要收起：窗口先留着，让页面播完收起、等提示条消失后放手（`belt::release`）再藏。
/// 页面一直不放手时过 `RELEASE_SAFETY` 自己藏，那时又打开、探出了就不藏。
pub fn linger(app: &AppHandle) {
    let g = LINGER.fetch_add(1, Ordering::SeqCst) + 1;
    let a = app.clone();
    let spawned = std::thread::Builder::new()
        .name("belt-linger".into())
        .spawn(move || {
            std::thread::sleep(RELEASE_SAFETY);
            if LINGER.load(Ordering::SeqCst) == g && crate::belt::closed() && shown() {
                log::info!("传送带 页面没放手，超时收起窗口");
                hide(&a);
            }
        });
    if let Err(e) = spawned {
        log::warn!("传送带收起计时线程启动失败：{e}");
        hide(app);
    }
}

pub fn hide(app: &AppHandle) {
    // 显示、收起都在主线程上依次做，SHOWN 才不会和窗口实际状态错开
    on_main(app, |app| {
        // 决定藏起到这里之间又打开、探出了（显示也排在主线程上）：不藏
        if !crate::belt::closed() {
            return;
        }
        SHOWN.store(false, Ordering::SeqCst);
        stop_slide();
        HITS.lock().unwrap_or_else(PoisonError::into_inner).clear();
        let Some(w) = app.get_webview_window(LABEL) else {
            return;
        };
        if let Err(e) = w.hide() {
            log::warn!("收起传送带失败：{e}");
        }
        // Windows：经 show_no_activate 上屏时 tao 的标记仍是隐藏，上面的 hide() 不会真的隐藏
        #[cfg(windows)]
        match w.hwnd() {
            Ok(h) => xpaste_platform::hide_window(h.0 as isize),
            Err(e) => log::warn!("传送带取不到窗口句柄：{e}"),
        }
    });
}

/// 显示期间每 16 ms 采样一次光标，按页面报的能点的矩形切换点击穿透；收起时停下。
fn spawn_pointer(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("belt-pointer".into())
        .spawn(move || {
            let mut prev: Option<bool> = None;
            let mut seen = GEN.load(Ordering::SeqCst);
            loop {
                std::thread::sleep(TICK);
                if !SHOWN.load(Ordering::SeqCst) {
                    RUNNING.store(false, Ordering::SeqCst);
                    // 停下的瞬间又显示了：接着跑
                    if SHOWN.load(Ordering::SeqCst) && !RUNNING.swap(true, Ordering::SeqCst) {
                        continue;
                    }
                    return;
                }
                let g = GEN.load(Ordering::SeqCst);
                if g != seen {
                    seen = g;
                    prev = None;
                }
                let Some((x, y)) = xpaste_platform::pointer_position() else {
                    continue;
                };
                let ((ox, oy), k) = *FRAME.lock().unwrap_or_else(PoisonError::into_inner);
                let local = ((x - ox) / k, (y - oy) / k);
                let hit = hit_test(local, &HITS.lock().unwrap_or_else(PoisonError::into_inner));
                let button = xpaste_platform::mouse_button_down();
                float_drag::tick(&app, LABEL, &DRAG, &FRAME, (x, y), button);
                resize_tick(&app, x, button);
                let want = pass_through(hit, button, false, prev);
                if prev != Some(want) {
                    prev = Some(want);
                    on_main(&app, move |app| {
                        if let Some(w) = app.get_webview_window(LABEL)
                            && let Err(e) = w.set_ignore_cursor_events(want)
                        {
                            log::warn!("传送带设置点击穿透失败：{e}");
                        }
                    });
                }
            }
        });
    if let Err(e) = spawned {
        RUNNING.store(false, Ordering::SeqCst);
        log::warn!("传送带指针线程启动失败：{e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1440.0,
        h: 860.0,
    };

    #[test]
    fn rect_centered_above_dock() {
        let r = belt_rect(WORK);
        assert_eq!((r.w, r.x, r.h), (800.0, 320.0, 64.0 + TOP_ROOM));
        // 带身底边在工作区底边上方 16 px
        assert_eq!(r.y + r.h, 844.0);
    }

    #[test]
    fn rect_narrow_screen() {
        assert_eq!(belt_rect(Rect { w: 800.0, ..WORK }).w, 672.0);
    }

    #[test]
    fn belt_room_yields_to_mixer() {
        let work = Rect {
            x: 0.0,
            y: 0.0,
            w: 1280.0,
            h: 800.0,
        };
        // 没有搅拌机：同 belt_rect
        assert_eq!(belt_room(work, None, None).x, belt_rect(work).x);
        // 搅拌机本身左边在 996：带子右边界 980，宽 800，居中在 [0, 980]
        let r = belt_room(work, Some(996.0), None);
        assert_eq!((r.x, r.w), (90.0, 800.0));
        // 调过的长度 1200 放不下：临时缩到 980 − 32
        assert_eq!(belt_room(work, Some(996.0), Some(1200.0)).w, 948.0);
        // 调过的长度 300 低于下限：360
        assert_eq!(belt_room(work, None, Some(300.0)).w, 360.0);
    }
}
