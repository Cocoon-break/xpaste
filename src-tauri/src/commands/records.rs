use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, State};
use xpaste_platform::PasteBlock;

use crate::error::AppError;
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(tag = "result", rename_all = "camelCase")]
pub enum CopyResult {
    /// 只复制不粘贴（阶段 4 的 copy 命令用）
    #[allow(dead_code)]
    Copied,
    CopiedOnly {
        reason: &'static str,
    },
    Pasted,
}

// 阶段 1 临时命令，阶段 4 由 copy 替换
/// 写剪切板；能模拟粘贴就隐藏面板并粘贴到上一个应用，否则只复制并说明原因（面板保持或重新显示）。
/// 阶段 4 的 copy 遵循同一规则：返回 `CopiedOnly` 时面板必须可见，用户才能看到降级提示。
#[tauri::command(async)]
pub async fn paste_text(
    app: AppHandle,
    state: State<'_, AppState>,
    text: String,
) -> Result<CopyResult, AppError> {
    state.writing.store(true, Ordering::SeqCst);
    let written = xpaste_platform::write_text(&text);
    if let Ok(seq) = written {
        state.last_write_seq.store(seq, Ordering::SeqCst);
    }
    state.writing.store(false, Ordering::SeqCst);
    written.map_err(|e| {
        log::warn!("写入剪切板失败 bytes={}：{e}", text.len());
        AppError::ClipboardWrite
    })?;
    let target = Some(state.previous_window.load(Ordering::Relaxed)).filter(|&h| h != 0);
    match xpaste_platform::can_paste(target) {
        PasteBlock::NeedAccessibility => Ok(CopyResult::CopiedOnly {
            reason: "needAccessibility",
        }),
        PasteBlock::Elevated => Ok(CopyResult::CopiedOnly { reason: "elevated" }),
        // Windows 不知道目标窗口时不盲发 Ctrl+V；面板尚未隐藏，直接给降级提示
        PasteBlock::None if cfg!(windows) && target.is_none() => {
            log::warn!("模拟粘贴跳过：没有记录到目标窗口");
            Ok(CopyResult::CopiedOnly {
                reason: "pasteFailed",
            })
        }
        PasteBlock::None => {
            crate::panel::hide(&app);
            match paste_on_main(&app, target).await {
                Ok(()) => {
                    log::info!("已模拟粘贴 bytes={}", text.len());
                    Ok(CopyResult::Pasted)
                }
                Err(e) => {
                    log::warn!("模拟粘贴失败：{e}");
                    crate::panel::reshow(&app);
                    Ok(CopyResult::CopiedOnly {
                        reason: "pasteFailed",
                    })
                }
            }
        }
    }
}

/// 等面板消失、焦点回到上一个应用后在主线程粘贴。sleep 与等待主线程都是阻塞调用，放到阻塞线程池。
async fn paste_on_main(app: &AppHandle, target: Option<isize>) -> Result<(), String> {
    let a = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        std::thread::sleep(Duration::from_millis(50));
        let (tx, rx) = mpsc::channel();
        a.run_on_main_thread(move || {
            let _ = tx.send(xpaste_platform::paste(target));
        })
        .map_err(|e| format!("切换到主线程失败：{e}"))?;
        rx.recv()
            .map_err(|_| "主线程未返回结果".to_string())?
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("阻塞任务失败：{e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn copy_result_matches_frontend_type() {
        let v = |r| serde_json::to_value(r).unwrap();
        assert_eq!(v(CopyResult::Copied), json!({"result": "copied"}));
        assert_eq!(v(CopyResult::Pasted), json!({"result": "pasted"}));
        assert_eq!(
            v(CopyResult::CopiedOnly {
                reason: "needAccessibility"
            }),
            json!({"result": "copiedOnly", "reason": "needAccessibility"})
        );
    }
}
