//! 发给前端的事件名（与 src/api/events.ts 同名）及发送助手。发送失败只记日志。

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

pub const PANEL_SHOWN: &str = "panel-shown";
pub const PANEL_HIDDEN: &str = "panel-hidden";
/// 快捷键 / 托盘要隐藏面板：前端播完淡出后调 `hide_panel`，400 ms 内没隐藏则后端兜底。
pub const PANEL_HIDE_REQUESTED: &str = "panel-hide-requested";
/// 引导或对话框打开时按下快捷键，面板保持显示：载荷 `"panel"`（唤起，前端据此推进引导、写入录制框）
/// 或 `"shot"` / `"rec"`（截图、录屏，见 `hotkey::overlay_pressed`）、`"translate"`（翻译，见 `hotkey::translate_pressed`）。
pub const HOTKEY_PRESSED: &str = "hotkey-pressed";
pub const RECORD_ADDED: &str = "record-added";
pub const RECORD_UPDATED: &str = "record-updated";
pub const RECORDS_REMOVED: &str = "records-removed";
pub const PAUSED_CHANGED: &str = "paused-changed";
/// 后端改了设置（录屏遮罩的声音开关写回，M24），发给面板，载荷是完整设置；面板换掉手里的副本，免得之后保存把它改回去。
pub const SETTINGS_CHANGED: &str = "settings-changed";
/// 托盘“设置…”：面板显示后打开设置弹窗。
pub const OPEN_SETTINGS: &str = "open-settings";
/// 托盘“清空历史…”：面板显示后打开清空确认。
pub const CONFIRM_CLEAR: &str = "confirm-clear";

/// 冻结完成，发给每块屏的遮罩 `shot-<i>`（载荷见 `shot::BeginPayload`）。
pub const SHOT_BEGIN: &str = "shot-begin";
/// 截图已结束，发给每块屏的遮罩；载荷是结束的那一次的代数，遮罩据此放掉冻结帧（M23 规则 11）。
pub const SHOT_END: &str = "shot-end";
/// 截图或录屏缺少屏幕录制权限，发给面板（M18）；载荷 `{ pane, mode }`：系统设置里那一页的名字（`screen_capture_pane`）与 `"shot"` / `"rec"`。
pub const SHOT_NEED_PERMISSION: &str = "shot-need-permission";

/// 录屏边框，发给 `rec-frame` 窗口：`{ x, y, w, h }`（该屏内逻辑像素）或 `null`（淡出）。
pub const REC_FRAME: &str = "rec-frame";

/// 翻译增量（M26 规则 3、5），载荷 `translate::Delta`：面板翻译时广播，截图翻译时只发给发起的遮罩窗口。
pub const TRANSLATE_DELTA: &str = "translate-delta";

pub fn emit<S: Serialize + Clone>(app: &AppHandle, event: &str, payload: S) {
    if let Err(e) = app.emit(event, payload) {
        log::warn!("发送 {event} 失败：{e}");
    }
}

/// 只发给指定标签的窗口。
pub fn emit_to<S: Serialize + Clone>(app: &AppHandle, label: &str, event: &str, payload: S) {
    if let Err(e) = app.emit_to(label, event, payload) {
        log::warn!("发送 {event} 给 {label} 失败：{e}");
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
