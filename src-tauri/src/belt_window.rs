//! 传送带窗口：贴在光标所在屏工作区的底边、水平居中，不拿键盘，透明处点击穿透。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::belt::{HITS, LABEL};
use crate::line::{hit_test, pass_through};
use crate::panel::{Rect, on_main};

pub const PAGE: &str = "pro/web/belt.html";

/// 带身高度
const BODY_H: f64 = 64.0;
/// 窗口比带身高出的部分，放提示条、探出提示和“下一件”标签；这一块透明处点击穿透
pub const TOP_ROOM: f64 = 96.0;
/// 带身底边离工作区底边
const BOTTOM_GAP: f64 = 16.0;
const TICK: Duration = Duration::from_millis(16);

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

/// 带子窗口正在显示（打开或探出），指针线程只在这期间跑
static SHOWN: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicBool = AtomicBool::new(false);
/// 每次显示加一：指针线程发现变了就忘掉上次设下的点击穿透，按光标重新设
static GEN: AtomicU64 = AtomicU64::new(0);
/// 窗口左上角和换成逻辑像素要除的系数，都在 `pointer_position` 的坐标空间里
/// （macOS 是点，系数 1；Windows 是物理像素，系数为所在屏缩放）
static FRAME: Mutex<((f64, f64), f64)> = Mutex::new(((0.0, 0.0), 1.0));

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

/// 摆到光标所在屏。主线程调用。
fn place(app: &AppHandle, w: &WebviewWindow) -> tauri::Result<()> {
    let Some(m) = crate::panel::cursor_monitor(app)? else {
        return Ok(());
    };
    let (sf, wa) = (m.scale_factor(), m.work_area());
    let r = belt_rect(Rect {
        x: wa.position.x as f64 / sf,
        y: wa.position.y as f64 / sf,
        w: wa.size.width as f64 / sf,
        h: wa.size.height as f64 / sf,
    });
    let k = if cfg!(target_os = "macos") { 1.0 } else { sf };
    *FRAME.lock().unwrap_or_else(PoisonError::into_inner) = ((r.x * k, r.y * k), k);
    crate::shot::place(w, (r.x * k, r.y * k), (r.w * k, r.h * k))
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
    });
}

pub fn hide(app: &AppHandle) {
    // 显示、收起都在主线程上依次做，SHOWN 才不会和窗口实际状态错开
    on_main(app, |app| {
        SHOWN.store(false, Ordering::SeqCst);
        HITS.lock().unwrap_or_else(PoisonError::into_inner).clear();
        if let Some(w) = app.get_webview_window(LABEL)
            && let Err(e) = w.hide()
        {
            log::warn!("收起传送带失败：{e}");
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
}
