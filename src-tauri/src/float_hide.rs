//! xPaste 自己截图、录屏、长截图时不把晾衣绳、传送带、搅拌机截进去，别的软件共享屏幕时照常看得见它们。
//!
//! - 截图冻结屏幕、长截图期间给在屏上的这几个窗口设内容保护（macOS `NSWindowSharingNone`，
//!   Windows `WDA_EXCLUDEFROMCAPTURE`），结束就去掉：常设的话用户共享屏幕时别人也看不见。
//! - macOS 录屏不设保护，把它们的窗口编号放进画面流的排除名单（`window_numbers`），浮层整场照常可见、能用。

use tauri::{AppHandle, Manager};

const LABELS: [&str; 3] = [crate::line::LABEL, crate::belt::LABEL, crate::mixer::LABEL];

/// 会话期间给晾衣绳、传送带、搅拌机设内容保护，drop 时复原（出错、取消也复原）
pub struct Hidden {
    app: AppHandle,
    labels: Vec<&'static str>,
}

impl Hidden {
    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }
}

/// 只对此刻在屏上的那几个设 set_content_protected(true)。
/// Tauri 的窗口设置都转到主线程执行：在主线程调用时立即生效，在别的线程调用时排进主线程队列，
/// 之后任何一次主线程往返（如 `rec::show_frame`）都排在它后面。
pub fn hide(app: &AppHandle) -> Hidden {
    let labels = on_screen(&LABELS.map(|l| (l, visible(app, l))));
    for l in &labels {
        protect(app, l, true);
    }
    if !labels.is_empty() {
        log::info!("float 截屏期间保护 {} 个浮层", labels.len());
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

fn visible(app: &AppHandle, label: &str) -> bool {
    app.get_webview_window(label)
        .is_some_and(|w| w.is_visible().unwrap_or(false))
}

fn protect(app: &AppHandle, label: &str, on: bool) {
    if let Some(w) = app.get_webview_window(label)
        && let Err(e) = w.set_content_protected(on)
    {
        log::warn!("float {label} 设内容保护（{on}）失败：{e}");
    }
}

/// 只取在屏上的
fn on_screen(labels: &[(&'static str, bool)]) -> Vec<&'static str> {
    labels.iter().filter(|(_, v)| *v).map(|(l, _)| *l).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_on_screen_floats_are_protected() {
        assert_eq!(
            on_screen(&[("line", true), ("belt", false), ("mixer", true)]),
            vec!["line", "mixer"]
        );
    }
}
