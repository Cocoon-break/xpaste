//! 剪切板监听接线：平台层回调 → 读快照 → 入库 → 事件 → 补全任务。
//! 日志只写字节数、敏感标记、来源应用名与记录 id。

use std::sync::atomic::Ordering;

use serde_json::json;
use tauri::{AppHandle, Manager};
use xpaste_core::capture::{self, Outcome};
use xpaste_core::{Kind, Snapshot, detect};

use crate::clock::now_ms;
use crate::enrich::{Job, icon_path};
use crate::events::{
    CLIPBOARD_CHANGED, RECORD_ADDED, RECORD_UPDATED, emit, emit_removed, emit_summary,
};
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
                let payload =
                    json!({ "bytes": s.text_bytes(), "sensitive": s.sensitive, "source": source });
                emit(&app, CLIPBOARD_CHANGED, payload);
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
        }
        Ok(Outcome::Added(id, removed)) => {
            log::info!("记录新增 id={id}");
            emit_summary(app, RECORD_ADDED, id);
            emit_removed(app, &removed);
            let dir = &state.data_dir;
            let mut jobs = Vec::new();
            if kind == Kind::Image {
                match first_path {
                    Some(path) => jobs.push(Job::ImageFile { id, path }),
                    None if dir.join("images").join(format!("{id}.png")).exists() => {
                        jobs.push(Job::Bitmap { id });
                    }
                    None => {}
                }
            }
            if let Some(app_id) = source
                && !icon_path(dir, &app_id).exists()
            {
                jobs.push(Job::AppIcon { app_id });
            }
            for job in jobs {
                if state.enrich.send(job).is_err() {
                    log::warn!("补全线程已退出");
                }
            }
        }
    }
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
