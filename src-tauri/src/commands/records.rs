use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, State};
use xpaste_core::RecordDetail;
use xpaste_platform::PasteBlock;

use crate::clock::now_ms;
use crate::error::AppError;
use crate::events::{RECORD_ADDED, RECORD_UPDATED, emit_removed, emit_summary};
use crate::state::AppState;

#[tauri::command(async)]
pub fn get_record(state: State<'_, AppState>, id: i64) -> Result<Option<RecordDetail>, AppError> {
    Ok(state.store().detail(id)?.map(crate::assets::detail_dto))
}

#[tauri::command(async)]
pub fn search(state: State<'_, AppState>, q: String) -> Result<Vec<i64>, AppError> {
    Ok(state.store().search_ids(&q)?)
}

#[tauri::command(async)]
pub fn set_pinned(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    on: bool,
) -> Result<(), AppError> {
    state.store().set_pinned(id, on)?;
    emit_summary(&app, RECORD_UPDATED, id);
    Ok(())
}

/// 软删除，5 秒内可撤销，之后彻底删除。
#[tauri::command(async)]
pub fn delete(app: AppHandle, state: State<'_, AppState>, id: i64) -> Result<(), AppError> {
    let now = now_ms();
    state.store().soft_delete(id, now)?;
    crate::housekeeping::schedule_purge(&app, id, now);
    emit_removed(&app, &[id]);
    Ok(())
}

#[tauri::command(async)]
pub fn undo_delete(app: AppHandle, state: State<'_, AppState>, id: i64) -> Result<bool, AppError> {
    let restored = state.store().undo_delete(id)?;
    if restored {
        emit_summary(&app, RECORD_ADDED, id);
    }
    Ok(restored)
}

/// 清空非置顶记录，返回删除条数。
#[tauri::command(async)]
pub fn clear_history(app: AppHandle, state: State<'_, AppState>) -> Result<u32, AppError> {
    let ids = state.store().clear_unpinned()?;
    emit_removed(&app, &ids);
    Ok(ids.len() as u32)
}

#[tauri::command(async)]
pub fn count_unpinned(state: State<'_, AppState>) -> Result<u32, AppError> {
    Ok(state.store().count_unpinned()?)
}

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
    match plan_paste(xpaste_platform::can_paste(target), target, cfg!(windows)) {
        PastePlan::CopyOnly(reason) => {
            if reason == REASON_PASTE_FAILED {
                log::warn!("模拟粘贴跳过：没有记录到目标窗口");
            }
            Ok(CopyResult::CopiedOnly { reason })
        }
        PastePlan::Paste => {
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
                        reason: REASON_PASTE_FAILED,
                    })
                }
            }
        }
    }
}

const REASON_PASTE_FAILED: &str = "pasteFailed";

/// 写入剪切板之后的决定：直接降级（面板保持可见），还是隐藏面板去粘贴。
#[derive(Debug, PartialEq, Eq)]
enum PastePlan {
    CopyOnly(&'static str),
    Paste,
}

fn plan_paste(block: PasteBlock, target: Option<isize>, windows: bool) -> PastePlan {
    match block {
        PasteBlock::NeedAccessibility => PastePlan::CopyOnly("needAccessibility"),
        PasteBlock::Elevated => PastePlan::CopyOnly("elevated"),
        // Windows 不知道目标窗口时不盲发 Ctrl+V（M09 失败处理：不粘贴到错误位置）
        PasteBlock::None if windows && target.is_none() => PastePlan::CopyOnly(REASON_PASTE_FAILED),
        PasteBlock::None => PastePlan::Paste,
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
    fn blocked_paste_degrades_with_matching_reason() {
        // AC-M09-05 / AC-M09-06：权限不足时只复制，原因与前端 CopyResult.reason 一致
        assert_eq!(
            plan_paste(PasteBlock::NeedAccessibility, None, false),
            PastePlan::CopyOnly("needAccessibility")
        );
        assert_eq!(
            plan_paste(PasteBlock::Elevated, Some(1), true),
            PastePlan::CopyOnly("elevated")
        );
    }

    #[test]
    fn windows_never_pastes_blind_but_macos_needs_no_target() {
        assert_eq!(
            plan_paste(PasteBlock::None, None, true),
            PastePlan::CopyOnly("pasteFailed")
        );
        assert_eq!(
            plan_paste(PasteBlock::None, Some(42), true),
            PastePlan::Paste
        );
        assert_eq!(plan_paste(PasteBlock::None, None, false), PastePlan::Paste);
    }

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
