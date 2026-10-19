//! 发给前端的事件名（与 src/api/events.ts 同名）及发送助手。发送失败只记日志。

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

pub const PANEL_SHOWN: &str = "panel-shown";
// 阶段 1 临时：前端计数用，阶段 3 移除
pub const CLIPBOARD_CHANGED: &str = "clipboard-changed";
pub const RECORD_ADDED: &str = "record-added";
pub const RECORD_UPDATED: &str = "record-updated";
pub const RECORDS_REMOVED: &str = "records-removed";
pub const PAUSED_CHANGED: &str = "paused-changed";

pub fn emit<S: Serialize + Clone>(app: &AppHandle, event: &str, payload: S) {
    if let Err(e) = app.emit(event, payload) {
        log::warn!("发送 {event} 失败：{e}");
    }
}

/// 读出记录摘要（转成前端 URL）后发送。已软删除的记录不发，避免补全晚到时前端把它加回列表。
pub fn emit_summary(app: &AppHandle, event: &str, id: i64) {
    let summary = app.state::<AppState>().store().live_summary(id);
    match summary {
        Ok(Some(s)) => emit(app, event, crate::assets::summary_dto(s)),
        Ok(None) => log::debug!("记录 {id} 已删除或不存在，不发 {event}"),
        Err(e) => log::warn!("读取记录 {id} 失败：{e}"),
    }
}

pub fn emit_removed(app: &AppHandle, ids: &[i64]) {
    if !ids.is_empty() {
        emit(app, RECORDS_REMOVED, ids);
    }
}
