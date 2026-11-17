//! xPaste 自己录屏、长截图时不把晾衣绳、传送带、搅拌机录进去，别的软件共享屏幕时照常看得见它们。
//! 普通截图照常截进去：屏幕上看到什么就截到什么（用户 2026-11-17 改的，想截浮层本身）。
//!
//! - 长截图（`hide`）：整场给这几个窗口设内容保护（macOS `NSWindowSharingNone`，Windows `WDA_EXCLUDEFROMCAPTURE`），
//!   结束就去掉：常设的话用户共享屏幕时别人也看不见。macOS 画面流另把它们列进排除名单。
//! - macOS 录屏不设保护，把它们的窗口编号放进画面流的排除名单（`window_numbers`），浮层整场照常可见、能用。

use tauri::{AppHandle, Manager};

const LABELS: [&str; 3] = [crate::line::LABEL, crate::belt::LABEL, crate::mixer::LABEL];

/// 会话期间不让晾衣绳、传送带、搅拌机被截进去，drop 时复原（出错、取消也复原）
pub struct Hidden {
    app: AppHandle,
    labels: Vec<&'static str>,
}

/// 长截图整场：给已建好的这几个窗口都设 set_content_protected(true)，隐藏着的也设，期间打开的同样截不进去。
/// Tauri 的窗口设置都转到主线程执行：在主线程调用时立即生效，在别的线程调用时排进主线程队列，
/// 之后任何一次主线程往返（如 `rec::show_frame`）都排在它后面。
pub fn hide(app: &AppHandle) -> Hidden {
    // 已建好的窗口都设，隐藏着的也设
    let labels: Vec<&'static str> = LABELS
        .into_iter()
        .filter(|&l| app.get_webview_window(l).is_some())
        .collect();
    for l in &labels {
        protect(app, l, true);
    }
    if !labels.is_empty() {
        log::info!("float 长截图期间保护 {} 个浮层", labels.len());
    }
    Hidden {
        app: app.clone(),
        labels,
    }
}

impl Drop for Hidden {
    /// 逐个 set_content_protected(false)，由 Tauri 转到主线程做
    fn drop(&mut self) {
        for l in &self.labels {
            protect(&self.app, l, false);
        }
    }
}

/// macOS：这几个浮层的窗口编号，给录屏排除用。隐藏着但上过屏的也算（编号不变），录制中途再显示也不录进去；
/// 录制开始后才第一次上屏的不在里面。其他平台为空。
pub fn window_numbers(app: &AppHandle) -> Vec<u32> {
    #[cfg(target_os = "macos")]
    {
        use std::{sync::mpsc, time::Duration};
        // 同 rec::show_frame：在主线程上读窗口编号，主线程卡住时不一直等
        let (tx, rx) = mpsc::channel();
        crate::panel::on_main(app, move |app| {
            let ids = LABELS
                .iter()
                .filter_map(|l| app.get_webview_window(l)?.ns_window().ok())
                .filter_map(xpaste_platform::window_number)
                .collect::<Vec<_>>();
            let _ = tx.send(ids);
        });
        rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Vec::new()
    }
}

fn protect(app: &AppHandle, label: &str, on: bool) {
    if let Some(w) = app.get_webview_window(label)
        && let Err(e) = w.set_content_protected(on)
    {
        log::warn!("float {label} 设内容保护（{on}）失败：{e}");
    }
}
