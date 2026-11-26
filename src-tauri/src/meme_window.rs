//! 表情包制作窗口：出现在光标所在屏的中央，拿键盘、能打字。
//! macOS 上是不激活应用、能成为 key 的 NSPanel（做法同面板），前台应用不变；Windows 上是普通的可聚焦窗口，关掉时把前台交还给打开前的窗口。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::events::{MEME_CLOSED, MEME_OPEN, MEME_TOAST, emit_to};
use crate::meme::{FULL, LABEL, OpenPayload};
use crate::panel::{Rect, on_main};

pub const PAGE: &str = "pro/web/meme.html";

pub const WIN_W: f64 = 960.0;
pub const WIN_H: f64 = 620.0;

/// 工作区中央；工作区比窗口小时贴左上
pub fn centered(work: Rect) -> Rect {
    let x = (work.x + (work.w - WIN_W) / 2.0).max(work.x);
    let y = (work.y + (work.h - WIN_H) / 2.0).max(work.y);
    Rect {
        x,
        y,
        w: WIN_W,
        h: WIN_H,
    }
}

/// 建好隐藏的窗口，免得打开时现建 webview、页面还没听事件。
pub fn prewarm(app: &AppHandle) {
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(PAGE.into()))
        .title("xPaste 表情包制作")
        .theme(crate::panel::current_theme(app))
        .visible(false)
        .focused(false)
        .transparent(true)
        .decorations(false)
        .resizable(false)
        .minimizable(false)
        .maximizable(false)
        .accept_first_mouse(true)
        .inner_size(WIN_W, WIN_H)
        .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
        .build();
    match built {
        // 普通窗口层级：切到别的应用时让开，输入法候选窗显示在它上面
        #[cfg(target_os = "macos")]
        Ok(w) => on_main(app, move |_| {
            if let Err(e) = crate::shot::to_overlay_panel(&w, 0) {
                log::warn!("表情包窗口转换 NSPanel 失败：{e}");
            }
        }),
        #[cfg(not(target_os = "macos"))]
        Ok(_) => {}
        Err(e) => log::warn!("创建表情包窗口失败：{e}"),
    }
}

/// 摆到光标所在屏的中央。主线程调用。
fn place(app: &AppHandle, w: &WebviewWindow) -> tauri::Result<()> {
    let Some((work, k)) = crate::float_drag::cursor_work(app)? else {
        return Ok(());
    };
    let r = centered(work);
    crate::shot::place(w, (r.x * k, r.y * k), (r.w * k, r.h * k))
}

/// 显示并拿键盘，再把载荷发给页面。`fresh` 时先摆到光标所在屏中央（已开着的不动，用户可能拖过）；`full` 时提示参考图满了。
pub fn show(app: &AppHandle, payload: OpenPayload, fresh: bool, full: bool) {
    on_main(app, move |app| {
        let Some(w) = app.get_webview_window(LABEL) else {
            log::warn!("找不到表情包窗口");
            return;
        };
        if fresh && let Err(e) = place(app, &w) {
            log::warn!("表情包窗口定位失败：{e}");
        }
        if let Err(e) = crate::trcard::front(app, &w, true) {
            log::warn!("显示表情包窗口失败：{e}");
        }
        // 页面还没把事件听上（应用刚启动）：先留着，等它说好了再发，不然窗口上了屏、拿了键盘却是空的
        if !READY.load(Ordering::SeqCst) {
            *PENDING.lock().unwrap_or_else(PoisonError::into_inner) = Some((payload, full));
            return;
        }
        send(app, payload, full);
    });
}

/// 页面把事件都听上了没有；没有时最近一次的打开载荷留在 `PENDING`（都只在主线程上读写）
static READY: AtomicBool = AtomicBool::new(false);
static PENDING: Mutex<Option<(OpenPayload, bool)>> = Mutex::new(None);

fn send(app: &AppHandle, payload: OpenPayload, full: bool) {
    emit_to(app, LABEL, MEME_OPEN, payload);
    if full {
        emit_to(app, LABEL, MEME_TOAST, FULL);
    }
}

/// 页面说事件都听上了：补发留着的那次打开
pub fn ready(app: &AppHandle) {
    on_main(app, |app| {
        READY.store(true, Ordering::SeqCst);
        let held = PENDING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some((payload, full)) = held {
            send(app, payload, full);
        }
    });
}

/// 藏起窗口，告诉页面清空。`back` 是打开前的前台窗口（只有 Windows 有）：趁窗口还在前台切回去（本进程此刻是前台进程，系统允许）；
/// macOS 上 NSPanel 撤下时系统把键盘交回前台应用。
pub fn hide(app: &AppHandle, back: Option<isize>) {
    on_main(app, move |app| {
        if let Some(h) = back {
            xpaste_platform::activate_window(h);
        }
        if let Some(w) = app.get_webview_window(LABEL)
            && let Err(e) = w.hide()
        {
            log::warn!("收起表情包窗口失败：{e}");
        }
        emit_to(app, LABEL, MEME_CLOSED, ());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_on_work_area() {
        let r = centered(Rect {
            x: 1440.0,
            y: 25.0,
            w: 1440.0,
            h: 875.0,
        });
        assert_eq!((r.x, r.y, r.w, r.h), (1680.0, 152.5, 960.0, 620.0));
        // 小屏贴左上
        let r = centered(Rect {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
        });
        assert_eq!((r.x, r.y), (0.0, 0.0));
    }
}
