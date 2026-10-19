//! 剪切板监听接线：平台层回调 → 读快照 → 日志 + 事件。日志只写字节数、敏感标记与来源应用名。

use std::sync::atomic::Ordering;

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

pub const CLIPBOARD_CHANGED: &str = "clipboard-changed";

/// 这次变化是否由 xPaste 自己写入引起（M01 规则 5）。
/// `writing`：正在写入中（Windows 上 WM_CLIPBOARDUPDATE 可能早于 last_write 写入到达）；
/// `last_write`：上次写入后的序号，`u64::MAX` 表示从未写过。
pub fn is_self_write(seq: u64, last_write: u64, writing: bool) -> bool {
    writing || seq == last_write
}

pub fn start(app: AppHandle) {
    xpaste_platform::watch(move |seq| {
        let state = app.state::<AppState>();
        if is_self_write(
            seq,
            state.last_write_seq.load(Ordering::SeqCst),
            state.writing.load(Ordering::SeqCst),
        ) {
            log::info!("跳过自身写入 seq={seq}");
            return;
        }
        match xpaste_platform::read_snapshot() {
            Err(e) => log::warn!("读取剪切板失败：{e}"),
            Ok(None) => log::debug!("剪切板为空"),
            Ok(Some(s)) => {
                let source = s.source.as_ref().map(|a| a.name.as_str());
                log::info!(
                    "剪切板变化 bytes={} sensitive={} source={:?}",
                    s.text_bytes(),
                    s.sensitive,
                    source
                );
                let payload =
                    json!({ "bytes": s.text_bytes(), "sensitive": s.sensitive, "source": source });
                if let Err(e) = app.emit(CLIPBOARD_CHANGED, payload) {
                    log::warn!("发送 {CLIPBOARD_CHANGED} 失败：{e}");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::is_self_write;

    #[test]
    fn skips_while_writing_or_on_matching_seq() {
        assert!(is_self_write(5, 1, true));
        assert!(is_self_write(7, 7, false));
    }

    #[test]
    fn never_written_sentinel_does_not_match_seq_zero() {
        assert!(!is_self_write(0, u64::MAX, false));
        assert!(!is_self_write(8, 7, false));
    }
}
