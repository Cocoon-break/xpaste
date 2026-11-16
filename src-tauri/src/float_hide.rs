//! xPaste 自己截图、录屏、长截图时不把晾衣绳、传送带、搅拌机截进去，别的软件共享屏幕时照常看得见它们。
//!
//! - 截图冻结（`hide_for_freeze`）：macOS 上把在屏上的这几个窗口临时设成全透明，冻结图拿到就复原。
//!   不靠内容保护：macOS 15 起截屏走 ScreenCaptureKit，不理会 `NSWindowSharingNone`。Windows 上设内容保护。
//! - 长截图（`hide`）：整场给这几个窗口设内容保护（macOS `NSWindowSharingNone`，Windows `WDA_EXCLUDEFROMCAPTURE`），
//!   结束就去掉：常设的话用户共享屏幕时别人也看不见。macOS 画面流另把它们列进排除名单。
//! - macOS 录屏不设保护，把它们的窗口编号放进画面流的排除名单（`window_numbers`），浮层整场照常可见、能用。

use tauri::{AppHandle, Manager};

const LABELS: [&str; 3] = [crate::line::LABEL, crate::belt::LABEL, crate::mixer::LABEL];

/// 会话期间不让晾衣绳、传送带、搅拌机被截进去，drop 时复原（出错、取消也复原）
pub struct Hidden {
    app: AppHandle,
    labels: Vec<&'static str>,
    /// macOS 冻结时设的是全透明（复原到 1.0）；false 表示设的是内容保护
    faded: bool,
}

impl Hidden {
    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }
}

/// 长截图整场：给已建好的这几个窗口都设 set_content_protected(true)，隐藏着的也设，期间打开的同样截不进去。
/// Tauri 的窗口设置都转到主线程执行：在主线程调用时立即生效，在别的线程调用时排进主线程队列，
/// 之后任何一次主线程往返（如 `rec::show_frame`）都排在它后面。
pub fn hide(app: &AppHandle) -> Hidden {
    let labels = targets(&windows(app), false);
    for l in &labels {
        protect(app, l, true);
    }
    if !labels.is_empty() {
        log::info!("float 长截图期间保护 {} 个浮层", labels.len());
    }
    Hidden {
        app: app.clone(),
        labels,
        faded: false,
    }
}

/// 截图冻结那一下：只动此刻在屏上的。macOS 设全透明（主线程调用），Windows 设内容保护。
pub fn hide_for_freeze(app: &AppHandle) -> Hidden {
    let labels = targets(&windows(app), true);
    #[cfg(target_os = "macos")]
    let faded = {
        for l in &labels {
            set_alpha(app, l, 0.0);
        }
        true
    };
    #[cfg(not(target_os = "macos"))]
    let faded = {
        for l in &labels {
            protect(app, l, true);
        }
        false
    };
    if !labels.is_empty() {
        log::info!("float 冻结屏幕时藏起 {} 个浮层", labels.len());
    }
    Hidden {
        app: app.clone(),
        labels,
        faded,
    }
}

impl Drop for Hidden {
    /// 内容保护逐个 set_content_protected(false)，由 Tauri 转到主线程做；alpha 切到主线程一律改回 1.0
    /// （两次冻结重叠时后一次读到的是 0，不能“还原”成 0）
    fn drop(&mut self) {
        if !self.faded {
            for l in &self.labels {
                protect(&self.app, l, false);
            }
            return;
        }
        #[cfg(target_os = "macos")]
        {
            let labels = std::mem::take(&mut self.labels);
            crate::panel::on_main(&self.app, move |app| {
                for l in &labels {
                    set_alpha(app, l, 1.0);
                }
            });
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

/// 已建好的浮层窗口与它此刻是否在屏上
fn windows(app: &AppHandle) -> Vec<(&'static str, bool)> {
    LABELS
        .iter()
        .filter_map(|&l| {
            let w = app.get_webview_window(l)?;
            Some((l, w.is_visible().unwrap_or(false)))
        })
        .collect()
}

/// 要处理的浮层：冻结只取在屏上的，内容保护取全部
fn targets(wins: &[(&'static str, bool)], freeze: bool) -> Vec<&'static str> {
    wins.iter()
        .filter(|(_, visible)| !freeze || *visible)
        .map(|(l, _)| *l)
        .collect()
}

fn protect(app: &AppHandle, label: &str, on: bool) {
    if let Some(w) = app.get_webview_window(label)
        && let Err(e) = w.set_content_protected(on)
    {
        log::warn!("float {label} 设内容保护（{on}）失败：{e}");
    }
}

/// 主线程调用：设窗口 alpha。这几个浮层自己不动窗口 alpha（只有面板做原生淡入淡出），所以复原一律用 1.0。
#[cfg(target_os = "macos")]
// 同 panel::animate_alpha：cocoa 已弃用；objc 0.2 的 sel! 展开里有 cfg(feature = "cargo-clippy")
#[allow(deprecated, unexpected_cfgs)]
fn set_alpha(app: &AppHandle, label: &str, to: f64) {
    use tauri_nspanel::cocoa::base::id;
    use tauri_nspanel::objc::{msg_send, sel, sel_impl};
    let Some(ns) = app
        .get_webview_window(label)
        .and_then(|w| w.ns_window().ok())
    else {
        return;
    };
    // SAFETY: 主线程上写自己的 NSWindow 的 alphaValue
    unsafe {
        let _: () = msg_send![ns as id, setAlphaValue: to];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freeze_takes_visible_floats_protection_takes_all() {
        let wins = [("line", true), ("belt", false), ("mixer", true)];
        assert_eq!(targets(&wins, true), vec!["line", "mixer"]);
        assert_eq!(targets(&wins, false), vec!["line", "belt", "mixer"]);
    }
}
