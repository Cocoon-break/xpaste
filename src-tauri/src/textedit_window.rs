//! 改字窗口：出现在光标所在屏的中央，拿键盘、能打字。
//! macOS 上是不激活应用、能成为 key 的 NSPanel（做法同面板），前台应用不变；Windows 上是普通的可聚焦窗口，关掉时把前台交还给打开前的窗口。
//! 打开的载荷由 `textedit` 在窗口显示之后发给页面，页面没就绪时先留着。

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::events::{TE_CLOSED, emit_to};
use crate::panel::{Rect, on_main};
use crate::textedit::LABEL;

const PAGE: &str = "pro/web/textedit.html";

const WIN_W: f64 = 1100.0;
const WIN_H: f64 = 720.0;

/// 工作区中央；工作区比窗口小时缩到工作区里（底栏的按钮不能落到屏幕外）
fn centered(work: Rect) -> Rect {
    let (w, h) = (WIN_W.min(work.w), WIN_H.min(work.h));
    Rect {
        x: work.x + (work.w - w) / 2.0,
        y: work.y + (work.h - h) / 2.0,
        w,
        h,
    }
}

/// 建好隐藏的窗口，免得打开时现建 webview、页面还没听事件。
pub fn prewarm(app: &AppHandle) {
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(PAGE.into()))
        .title("xPaste 改字")
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
        Ok(w) => on_main(app, move |_| match crate::shot::to_overlay_panel(&w, 0) {
            Ok(()) => log::info!("改字窗口已建好"),
            Err(e) => log::warn!("改字窗口转换 NSPanel 失败：{e}"),
        }),
        #[cfg(not(target_os = "macos"))]
        Ok(_) => log::info!("改字窗口已建好"),
        Err(e) => log::warn!("创建改字窗口失败：{e}"),
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

/// 显示并拿键盘。`fresh` 时先摆到光标所在屏中央（已开着的不动，用户可能拖过）。主线程调用。
pub fn show(app: &AppHandle, fresh: bool) {
    let Some(w) = app.get_webview_window(LABEL) else {
        log::warn!("找不到改字窗口");
        return;
    };
    if fresh && let Err(e) = place(app, &w) {
        log::warn!("改字窗口定位失败：{e}");
    }
    if let Err(e) = crate::trcard::front(app, &w, true) {
        log::warn!("显示改字窗口失败：{e}");
    }
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
            log::warn!("收起改字窗口失败：{e}");
        }
        emit_to(app, LABEL, TE_CLOSED, ());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_and_fits_work_area() {
        let at = |x, y, w, h| {
            let r = centered(Rect { x, y, w, h });
            (r.x, r.y, r.w, r.h)
        };
        assert_eq!(
            at(1440.0, 25.0, 1440.0, 875.0),
            (1610.0, 102.5, 1100.0, 720.0)
        );
        // 1920×1080、150% 缩放：工作区 1280×672
        assert_eq!(at(0.0, 0.0, 1280.0, 672.0), (90.0, 0.0, 1100.0, 672.0));
        assert_eq!(at(10.0, 20.0, 800.0, 600.0), (10.0, 20.0, 800.0, 600.0));
    }
}
