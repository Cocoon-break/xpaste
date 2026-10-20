use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::{LazyLock, Mutex, PoisonError, mpsc};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use xpaste_core::{Flavor, ImageInfo, Kind, RecordDetail, SearchHit, ocr_state};
use xpaste_platform::{PasteBlock, PlatformError};

use crate::clock::now_ms;
use crate::enrich::Job;
use crate::error::AppError;
use crate::events::{RECORD_ADDED, RECORD_UPDATED, emit_removed, emit_summary};
use crate::state::AppState;

/// 图片文件引用（不是位图）与视频的预览：只授权这一个源文件给 asset 协议（设计 9.5）。
#[tauri::command(async)]
pub fn get_record(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<Option<RecordDetail>, AppError> {
    let Some(mut d) = state.store().detail(id)?.map(crate::assets::detail_dto) else {
        return Ok(None);
    };
    if d.summary.kind == Kind::Image
        && d.image.is_none()
        && let Some(f) = d.files.first().filter(|f| f.exists)
    {
        let path = std::path::Path::new(&f.path);
        app.asset_protocol_scope().allow_file(path)?;
        // 补全线程只给位图写尺寸，文件引用读文件头；HEIC 等读不出时为 0
        let (w, h) = image::ImageReader::open(path)
            .and_then(|r| r.with_guessed_format())
            .ok()
            .and_then(|r| r.into_dimensions().ok())
            .unwrap_or((0, 0));
        d.image = Some(ImageInfo {
            src: crate::assets::asset_url(path),
            w,
            h,
        });
    }
    // 视频同理：只授权该视频源文件，供详情 <video> 播放
    if d.video.as_ref().is_some_and(|v| v.src.is_some()) {
        app.asset_protocol_scope()
            .allow_file(std::path::Path::new(&d.files[0].path))?;
    }
    // 每个 id 每次运行最多补跑一次：AVFoundation 读不了的格式不会写 meta，否则每次选中都会重排
    static BACKFILLED: LazyLock<Mutex<HashSet<i64>>> = LazyLock::new(Default::default);
    if needs_video_backfill(&d)
        && BACKFILLED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id)
    {
        let path = d.files[0].path.clone().into();
        if state.enrich.send(Job::Video { id, path }).is_err() {
            log::warn!("补全线程已退出");
        }
    }
    Ok(Some(d))
}

/// 阶段 7 之前采集或去重命中的视频没有补全过：源文件还在、还没有时长（`meta.duration`）时补跑。
fn needs_video_backfill(d: &RecordDetail) -> bool {
    d.summary.kind == Kind::Video
        && d.summary.duration_sec.is_none()
        && d.files.first().is_some_and(|f| f.exists)
}

#[tauri::command(async)]
pub fn search(state: State<'_, AppState>, q: String) -> Result<Vec<SearchHit>, AppError> {
    Ok(state.store().search_hits(&q)?)
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

/// 重新识别：只对失败、跳过、本机不支持的记录生效，置回待识别并插队（面板可见也立即开始）；其他状态什么也不做。
#[tauri::command(async)]
pub fn reocr(app: AppHandle, state: State<'_, AppState>, id: i64) -> Result<(), AppError> {
    let reset = {
        let store = state.store();
        matches!(
            store.ocr_state(id)?,
            Some(ocr_state::FAILED | ocr_state::SKIPPED | ocr_state::UNSUPPORTED)
        ) && store.set_ocr(id, ocr_state::PENDING, None)?
    };
    if reset {
        emit_summary(&app, RECORD_UPDATED, id);
        state.ocr.push_forced(id);
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrProgress {
    pub done: u32,
    pub total: u32,
    pub supported: bool,
}

#[tauri::command(async)]
pub fn ocr_progress(state: State<'_, AppState>) -> Result<OcrProgress, AppError> {
    let (done, total) = state.store().ocr_progress()?;
    Ok(OcrProgress {
        done,
        total,
        supported: state.ocr.supported,
    })
}

#[derive(Debug, Serialize)]
#[serde(tag = "result", rename_all = "camelCase")]
pub enum CopyResult {
    Copied,
    CopiedOnly { reason: &'static str },
    Pasted,
}

/// 写回记录的原始 flavor；`paste` 时能模拟粘贴就隐藏面板并粘贴到上一个应用，
/// 否则只复制并说明原因。返回 `CopiedOnly` 时面板必须可见，用户才能看到降级提示（设计 8.5）。
#[tauri::command(async)]
pub async fn copy(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    paste: bool,
) -> Result<CopyResult, AppError> {
    let flavors = precheck(&state, id)?;
    let bytes: usize = flavors.iter().map(|f| f.data.len()).sum();
    write_marked(&state, || xpaste_platform::write(&flavors)).inspect_err(|_| {
        log::warn!("写入剪切板失败 id={id} bytes={bytes}");
    })?;
    let move_to_top = state.settings().move_to_top;
    state.store().bump_use(id, move_to_top, now_ms())?;
    emit_summary(&app, RECORD_UPDATED, id);
    if !paste {
        return Ok(CopyResult::Copied);
    }
    let target = Some(state.previous_window.load(Ordering::Relaxed)).filter(|&h| h != 0);
    match plan_paste(xpaste_platform::can_paste(target), target, cfg!(windows)) {
        PastePlan::CopyOnly(reason) => {
            if reason == REASON_PASTE_FAILED {
                // 从托盘打开后按 Enter 是正常路径，不算警告
                log::info!("模拟粘贴跳过：没有记录到目标窗口");
            }
            Ok(CopyResult::CopiedOnly { reason })
        }
        PastePlan::Paste => {
            crate::panel::hide(&app);
            match paste_on_main(&app, target).await {
                Ok(()) => {
                    log::info!("已模拟粘贴 id={id} bytes={bytes}");
                    Ok(CopyResult::Pasted)
                }
                Err(e) => {
                    log::warn!("模拟粘贴失败 id={id}：{e}");
                    crate::panel::reshow(&app);
                    Ok(CopyResult::CopiedOnly {
                        reason: REASON_PASTE_FAILED,
                    })
                }
            }
        }
    }
}

/// 预览“复制文字”：只写一份纯文本，标记为自身写入（不入库、不计使用次数）。
#[tauri::command(async)]
pub fn copy_text_plain(state: State<'_, AppState>, text: String) -> Result<(), AppError> {
    let flavor = if cfg!(windows) {
        // 存储口径：UTF-16LE、不带结尾 NUL（写回时 winfmt::write_bytes 补上）
        Flavor {
            item: 0,
            format: "CF_UNICODETEXT".into(),
            data: text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        }
    } else {
        Flavor {
            item: 0,
            format: "public.utf8-plain-text".into(),
            data: text.into_bytes(),
        }
    };
    write_marked(&state, || xpaste_platform::write(&[flavor]))
}

/// 设计 8.5 第 1–2 步：记录存在、文件都在、有可写回的 flavor。任何一步不满足都不碰剪切板。
fn precheck(state: &AppState, id: i64) -> Result<Vec<Flavor>, AppError> {
    let store = state.store();
    let detail = store
        .detail(id)?
        .ok_or_else(|| AppError::Invalid("记录不存在".into()))?;
    if detail.files.iter().any(|f| !f.exists) {
        return Err(AppError::FileMissing);
    }
    let flavors = store.flavors(id)?;
    if flavors.is_empty() {
        return Err(AppError::ClipboardWrite);
    }
    Ok(flavors)
}

/// 写剪切板期间置 `writing`，成功后记下序号，让监听线程跳过这次自身写入。
fn write_marked(
    state: &AppState,
    f: impl FnOnce() -> Result<u64, PlatformError>,
) -> Result<(), AppError> {
    state.writing.store(true, Ordering::SeqCst);
    let written = f();
    if let Ok(seq) = written {
        state.last_write_seq.store(seq, Ordering::SeqCst);
    }
    state.writing.store(false, Ordering::SeqCst);
    written.map(drop).map_err(|e| {
        log::warn!("写剪切板出错：{e}");
        AppError::ClipboardWrite
    })
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
    use xpaste_core::{Snapshot, capture::build_record, detect};

    fn insert(state: &AppState, snap: Snapshot) -> i64 {
        let kind = detect::kind(&snap);
        let (up, _) = state
            .store()
            .insert_or_merge(&build_record(snap, kind, 0), 0, 500)
            .unwrap();
        match up {
            xpaste_core::store::Upsert::Added(id) | xpaste_core::store::Upsert::Merged(id) => id,
        }
    }

    fn insert_file_record(state: &AppState, path: &str) -> i64 {
        insert(
            state,
            Snapshot {
                file_paths: vec![path.into()],
                ..Default::default()
            },
        )
    }

    /// 平台层采集时总带 flavor；这里只给文本，入库后没有 flavor 行。
    fn insert_text_record_without_flavors(state: &AppState) -> i64 {
        insert(
            state,
            Snapshot {
                text: Some("x".into()),
                ..Default::default()
            },
        )
    }

    #[test]
    fn copy_refuses_when_a_file_is_missing() {
        let state = AppState::for_test();
        let id = insert_file_record(&state, "/nonexistent/xpaste-missing.txt");
        assert!(matches!(precheck(&state, id), Err(AppError::FileMissing)));
    }

    #[test]
    fn copy_refuses_record_without_flavors() {
        let state = AppState::for_test();
        let id = insert_text_record_without_flavors(&state);
        assert!(matches!(
            precheck(&state, id),
            Err(AppError::ClipboardWrite)
        ));
    }

    #[test]
    fn video_backfill_only_for_existing_videos_without_duration() {
        let state = AppState::for_test();
        let f = std::env::temp_dir().join(format!("xpaste-backfill-{}.mp4", std::process::id()));
        std::fs::write(&f, b"x").unwrap();
        let id = insert_file_record(&state, f.to_str().unwrap());
        let d = |id| state.store().detail(id).unwrap().unwrap();
        assert!(needs_video_backfill(&d(id)));
        state
            .store()
            .set_meta(id, &json!({"duration": 3.0}))
            .unwrap();
        assert!(!needs_video_backfill(&d(id)));
        assert!(!needs_video_backfill(&d(insert_file_record(
            &state,
            "/definitely/missing.mp4"
        ))));
        assert!(!needs_video_backfill(&d(
            insert_text_record_without_flavors(&state)
        )));
        std::fs::remove_file(&f).unwrap();
    }

    #[test]
    fn copy_refuses_unknown_record() {
        let state = AppState::for_test();
        assert!(matches!(precheck(&state, 999), Err(AppError::Invalid(_))));
    }

    #[test]
    fn write_marked_sets_sequence_and_clears_writing_on_error() {
        let state = AppState::for_test();
        let r = write_marked(&state, || Err(PlatformError::Unsupported));
        assert!(matches!(r, Err(AppError::ClipboardWrite)));
        assert!(!state.writing.load(Ordering::SeqCst));
        assert_eq!(state.last_write_seq.load(Ordering::SeqCst), u64::MAX);
        write_marked(&state, || Ok(7)).unwrap();
        assert_eq!(state.last_write_seq.load(Ordering::SeqCst), 7);
    }

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
