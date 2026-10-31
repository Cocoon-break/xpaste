//! 剪切板监听接线：平台层回调 → 读快照 → 入库 → 事件 → 补全任务。
//! 日志只写字节数、敏感标记、来源应用名与记录 id。

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use tauri::{AppHandle, Manager};
use xpaste_core::capture::{self, Outcome};
use xpaste_core::{Kind, Snapshot, detect};

use crate::clock::now_ms;
use crate::enrich::{Job, icon_path};
use crate::events::{RECORD_ADDED, RECORD_UPDATED, emit_removed, emit_summary};
use crate::state::AppState;

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
        // 暂停或忽略的应用：连剪切板都不读（process 里仍有同样的检查兜底）
        if state.paused.load(Ordering::SeqCst) {
            log::info!("跳过 Paused");
            return;
        }
        let (ignored, skip_sensitive) = {
            let s = state.settings();
            (s.ignored_apps.clone(), s.skip_sensitive)
        };
        if xpaste_platform::frontmost_app()
            .is_some_and(|a| capture::is_ignored_app(&a.name, &ignored))
        {
            log::info!("跳过 IgnoredApp");
            return;
        }
        match xpaste_platform::read_snapshot(skip_sensitive) {
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
                ingest(&app, s);
            }
        }
    });
}

fn ingest(app: &AppHandle, snap: Snapshot) {
    let state = app.state::<AppState>();
    // process 会消耗快照，先记下补全任务要用的信息
    let kind = detect::kind(&snap);
    let first_path = snap.file_paths.first().cloned();
    // 文件列表里夹着图片的文件记录也识别文字（没有缩略图任务，直接入队）
    let ocr_files = kind == Kind::File && detect::has_image_file(&snap.file_paths);
    let source = snap.source.as_ref().map(|a| a.id.clone());
    let settings = state.settings().clone();
    let paused = state.paused.load(Ordering::SeqCst);
    let outcome = capture::process(snap, &settings, paused, &mut state.store(), now_ms());
    match outcome {
        Err(e) => log::warn!("记录入库失败：{e}"),
        Ok(Outcome::Skipped(reason)) => log::info!("跳过 {reason:?}"),
        Ok(Outcome::Merged(id, removed)) => {
            log::info!("记录更新 id={id}");
            emit_summary(app, RECORD_UPDATED, id);
            emit_removed(app, &removed);
            // 再次复制还没识别的图片（例如补识别关着）：插队识别，已识别的由识别线程丢弃
            if kind == Kind::Image || ocr_files {
                state.ocr.push_front(id);
            }
        }
        Ok(Outcome::Added(id, removed)) => {
            log::info!("记录新增 id={id}");
            emit_summary(app, RECORD_ADDED, id);
            emit_removed(app, &removed);
            enqueue_followups(app, id, kind, first_path, ocr_files);
            if let Some(app_id) = source
                && !icon_path(&state.data_dir, &app_id).exists()
                && state.enrich.send(Job::AppIcon { app_id }).is_err()
            {
                log::warn!("补全线程已退出");
            }
        }
    }
}

/// 新记录的后台任务：缩略图、视频封面与文字识别入队（不含应用图标）。
pub fn enqueue_followups(
    app: &AppHandle,
    id: i64,
    kind: Kind,
    first_path: Option<PathBuf>,
    ocr_files: bool,
) {
    let state = app.state::<AppState>();
    let job = match (kind, first_path) {
        (Kind::Image, Some(path)) => Some(Job::ImageFile { id, path }),
        (Kind::Image, None)
            if state
                .data_dir
                .join("images")
                .join(format!("{id}.png"))
                .exists() =>
        {
            Some(Job::Bitmap { id })
        }
        (Kind::Video, Some(path)) => Some(Job::Video { id, path }),
        _ => None,
    };
    if ocr_files {
        state.ocr.push_front(id);
    }
    if let Some(job) = job
        && state.enrich.send(job).is_err()
    {
        log::warn!("补全线程已退出");
    }
}

/// 按普通写入写纯文本（不打自身写入标记），监听器把它作为一条文本记录收进历史：截图识别文字、复制译文用。
/// 失败时日志只记 `what` 与字数，报“写剪切板失败”。
pub fn write_text(text: &str, what: &str) -> Result<(), crate::error::AppError> {
    xpaste_platform::write(&[xpaste_core::text_flavor(text)]).map_err(|e| {
        log::warn!("{what}写剪切板失败 {} 字：{e}", text.chars().count());
        crate::error::AppError::ClipboardWrite
    })?;
    Ok(())
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
