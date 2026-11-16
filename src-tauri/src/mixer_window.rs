//! 搅拌机窗口：立在光标所在屏工作区的右下角，平时不拿键盘，透明处点击穿透；补一句展开时才拿键盘，收起时交还。
//!
//! ponytail: 照 `belt_window.rs` 复制改写（不激活的浮层、摆屏、点击穿透线程、收起兜底），没抽公共模块；
//! 出现第三个这样的浮层时再把三处合并成一个。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::events::{MIXER_SHOWN, emit_to};
use crate::float_drag;
use crate::line::{hit_test, pass_through};
use crate::mixer::{HITS, LABEL};
use crate::panel::{Rect, on_main};

pub const PAGE: &str = "pro/web/mixer.html";

pub const WIN_W: f64 = 380.0;
pub const WIN_H: f64 = 600.0;
/// 窗口离工作区右边、下边
const GAP: f64 = 16.0;
const TICK: Duration = Duration::from_millis(16);
/// 要收起以后等页面放手（`mixer_release`）最多这么久，过了就自己藏
const RELEASE_SAFETY: Duration = Duration::from_secs(5);

/// 默认的窗口矩形（逻辑像素）：工作区右下角，离右边、下边各 16 px。拖过的照记下的位置摆（`float_drag`）
pub fn mixer_rect(work: Rect) -> Rect {
    Rect {
        x: work.x + work.w - GAP - WIN_W,
        y: work.y + work.h - GAP - WIN_H,
        w: WIN_W,
        h: WIN_H,
    }
}

/// 窗口在屏上（含收起后等页面放手的那一会儿），指针线程只在这期间跑
static SHOWN: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicBool = AtomicBool::new(false);
/// 每次显示加一：指针线程发现变了就忘掉上次设下的点击穿透，按光标重新设
static GEN: AtomicU64 = AtomicU64::new(0);
/// 每次要收起加一：超时藏窗口前看一眼，之后又要收起过的话这次作废
static LINGER: AtomicU64 = AtomicU64::new(0);
/// 窗口左上角和换成逻辑像素要除的系数，都在 `pointer_position` 的坐标空间里
/// （macOS 是点，系数 1；Windows 是物理像素，系数为所在屏缩放）
static FRAME: float_drag::Frame = Mutex::new(((0.0, 0.0), 1.0));
/// 正在拖动换位置
static DRAG: float_drag::Dragging = Mutex::new(None);
/// 补一句拿着键盘，和拿键盘前的前台窗口（只有 Windows 有）
static KEYBOARD: Mutex<(bool, Option<isize>)> = Mutex::new((false, None));

/// 建好隐藏的窗口，免得第一次显示时现建 webview。参数同传送带。
pub fn prewarm(app: &AppHandle) {
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(PAGE.into()))
        .title("xpaste-mixer")
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
        .inner_size(WIN_W, WIN_H)
        .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
        .build();
    match built {
        #[cfg(target_os = "macos")]
        Ok(w) => on_main(app, move |_| {
            // 同传送带的层级；全屏空间里也显示（to_overlay_panel 已带上）
            if let Err(e) = crate::shot::to_overlay_panel(&w, 2) {
                log::warn!("搅拌机窗口转换 NSPanel 失败：{e}");
            }
        }),
        #[cfg(windows)]
        Ok(w) => on_main(app, move |_| match w.hwnd() {
            Ok(h) => xpaste_platform::tool_window(h.0 as isize),
            Err(e) => log::warn!("搅拌机取不到窗口句柄：{e}"),
        }),
        #[cfg(not(any(target_os = "macos", windows)))]
        Ok(_) => {}
        Err(e) => log::warn!("创建搅拌机窗口失败：{e}"),
    }
}

/// 摆到光标所在屏：拖过的照记下的位置，没拖过放右下角。主线程调用。
fn place(app: &AppHandle, w: &WebviewWindow) -> tauri::Result<()> {
    let Some((work, k)) = float_drag::cursor_work(app)? else {
        return Ok(());
    };
    let r = float_drag::placed(work, mixer_rect(work), float_drag::saved(app, LABEL));
    *FRAME.lock().unwrap_or_else(PoisonError::into_inner) = ((r.x * k, r.y * k), k);
    crate::shot::place(w, (r.x * k, r.y * k), (r.w * k, r.h * k))
}

/// 页面在底座空白处按下：开始拖动换位置
pub fn drag() {
    if shown() {
        float_drag::start(&DRAG, &FRAME, (WIN_W, WIN_H));
    }
}

/// 显示搅拌机，不拿键盘。已经显示着就不动，关闭之前不跟着光标换屏。
pub fn show(app: &AppHandle) {
    on_main(app, |app| {
        let Some(w) = app.get_webview_window(LABEL) else {
            return;
        };
        if SHOWN.swap(true, Ordering::SeqCst) {
            return;
        }
        let shown = place(app, &w)
            .and_then(|()| w.set_ignore_cursor_events(true))
            .and_then(|()| crate::trcard::front(app, &w, false));
        if let Err(e) = shown {
            log::warn!("显示搅拌机失败：{e}");
            SHOWN.store(false, Ordering::SeqCst);
            return;
        }
        GEN.fetch_add(1, Ordering::SeqCst);
        if !RUNNING.swap(true, Ordering::SeqCst) {
            spawn_pointer(app);
        }
        // 藏起时清掉了点击区域，让页面重报
        emit_to(app, LABEL, MIXER_SHOWN, ());
    });
}

/// 搅拌机窗口在屏上（含收起后等页面放手的那一会儿）
pub fn shown() -> bool {
    SHOWN.load(Ordering::SeqCst)
}

/// 搅拌机要收起：窗口先留着，让页面播完收起、等提示条消失后放手（`mixer::release`）再藏。
/// 页面一直不放手时过 `RELEASE_SAFETY` 自己藏，那时又打开了就不藏。
pub fn linger(app: &AppHandle) {
    let g = LINGER.fetch_add(1, Ordering::SeqCst) + 1;
    let a = app.clone();
    let spawned = std::thread::Builder::new()
        .name("mixer-linger".into())
        .spawn(move || {
            std::thread::sleep(RELEASE_SAFETY);
            if LINGER.load(Ordering::SeqCst) == g && crate::mixer::closed() && shown() {
                log::info!("搅拌机 页面没放手，超时收起窗口");
                hide(&a);
            }
        });
    if let Err(e) = spawned {
        log::warn!("搅拌机收起计时线程启动失败：{e}");
        hide(app);
    }
}

pub fn hide(app: &AppHandle) {
    // 显示、收起都在主线程上依次做，SHOWN 才不会和窗口实际状态错开
    on_main(app, |app| {
        // 决定藏起到这里之间又打开了（显示也排在主线程上）：不藏
        if !crate::mixer::closed() {
            return;
        }
        SHOWN.store(false, Ordering::SeqCst);
        HITS.lock().unwrap_or_else(PoisonError::into_inner).clear();
        // 拿着键盘时交还：Windows 趁窗口还在前台切回原来的窗口（本进程此刻是前台进程，系统允许）；
        // macOS 窗口撤下时系统把键盘交回去
        let back = std::mem::take(&mut *KEYBOARD.lock().unwrap_or_else(PoisonError::into_inner));
        if let (true, Some(h)) = back {
            xpaste_platform::activate_window(h);
        }
        let Some(w) = app.get_webview_window(LABEL) else {
            return;
        };
        if let Err(e) = w.hide() {
            log::warn!("收起搅拌机失败：{e}");
        }
        // Windows：经 show_no_activate 上屏时 tao 的标记仍是隐藏，上面的 hide() 不会真的隐藏
        #[cfg(windows)]
        match w.hwnd() {
            Ok(h) => xpaste_platform::hide_window(h.0 as isize),
            Err(e) => log::warn!("搅拌机取不到窗口句柄：{e}"),
        }
    });
}

/// 补一句：`on` 时拿键盘（Windows 先记下此刻的前台窗口），`off` 时交还。
/// 交还：Windows 切回记下的窗口（那个窗口已关掉就不做别的）；macOS 撤下再上屏、不拿键盘，由系统把键盘交回原来的应用
/// （nonactivating panel 上 `resignKeyWindow` 只是通知，不会把键盘交出去；同晾衣绳一直显示时的做法）。
pub fn keyboard(app: &AppHandle, on: bool) {
    if on {
        if !shown() {
            return;
        }
        {
            let mut k = KEYBOARD.lock().unwrap_or_else(PoisonError::into_inner);
            if !k.0 {
                *k = (true, xpaste_platform::foreground_window());
            }
        }
        on_main(app, |app| {
            if let Some(w) = app.get_webview_window(LABEL) {
                if let Err(e) = crate::trcard::front(app, &w, true) {
                    log::warn!("搅拌机取得焦点失败：{e}");
                }
                log::info!("搅拌机 拿键盘 focused={}", w.is_focused().unwrap_or(false));
            }
        });
        return;
    }
    let (held, back) =
        std::mem::take(&mut *KEYBOARD.lock().unwrap_or_else(PoisonError::into_inner));
    log::info!("搅拌机 交还键盘 held={held} back={}", back.is_some());
    if !held {
        return;
    }
    on_main(app, move |app| {
        if let Some(h) = back {
            xpaste_platform::activate_window(h);
        }
        #[cfg(target_os = "macos")]
        if let Some(w) = app.get_webview_window(LABEL)
            && shown()
            && w.is_focused().unwrap_or(false)
            && let Err(e) = w.hide().and_then(|()| crate::trcard::front(app, &w, false))
        {
            log::warn!("搅拌机交还键盘失败：{e}");
        }
        #[cfg(not(target_os = "macos"))]
        let _ = app;
    });
}

/// 搅拌机窗口失去键盘（用户点了别的应用）：键盘已在用户点的地方，交还时不再切回原来的窗口。
pub fn blur() {
    *KEYBOARD.lock().unwrap_or_else(PoisonError::into_inner) = (false, None);
}

/// 显示期间每 16 ms 采样一次光标，按页面报的能点的矩形切换点击穿透；收起时停下。
fn spawn_pointer(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("mixer-pointer".into())
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
                // 拖着时窗口一直接收（按着键时 pass_through 本来就保持按下那一刻的状态）
                float_drag::tick(&app, LABEL, &DRAG, &FRAME, (x, y), button);
                let want = pass_through(hit, button, false, prev);
                if prev != Some(want) {
                    prev = Some(want);
                    if KEYBOARD.lock().unwrap_or_else(PoisonError::into_inner).0 {
                        log::info!("搅拌机 拿着键盘时点击穿透 {want}");
                    }
                    on_main(&app, move |app| {
                        if let Some(w) = app.get_webview_window(LABEL)
                            && let Err(e) = w.set_ignore_cursor_events(want)
                        {
                            log::warn!("搅拌机设置点击穿透失败：{e}");
                        }
                    });
                }
            }
        });
    if let Err(e) = spawned {
        RUNNING.store(false, Ordering::SeqCst);
        log::warn!("搅拌机指针线程启动失败：{e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect {
        x: 0.0,
        y: 25.0,
        w: 1440.0,
        h: 835.0,
    };

    #[test]
    fn rect_bottom_right() {
        let r = mixer_rect(WORK);
        assert_eq!((r.x, r.y, r.w, r.h), (1044.0, 244.0, 380.0, 600.0));
    }

    #[test]
    fn rect_offset_screen() {
        assert_eq!(mixer_rect(Rect { x: 1440.0, ..WORK }).x, 2484.0);
    }
}
