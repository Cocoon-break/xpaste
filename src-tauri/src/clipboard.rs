//! 剪切板监听接线：平台层回调 → 读快照 → 日志 + 事件。日志只写字节数、敏感标记与来源应用名。

use std::sync::atomic::Ordering;

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

pub const CLIPBOARD_CHANGED: &str = "clipboard-changed";

pub fn start(app: AppHandle) {
    xpaste_platform::watch(move |seq| {
        let state = app.state::<AppState>();
        // Windows 上 WM_CLIPBOARDUPDATE 可能早于 last_write_seq 写入到达
        if state.writing.load(Ordering::SeqCst) {
            log::info!("跳过自身写入（写入中）");
            return;
        }
        if seq == state.last_write_seq.load(Ordering::SeqCst) {
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
