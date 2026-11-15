//! 传送带页面与面板调用的命令。都在后台线程跑：要读库、写剪切板，不占主线程。

use tauri::AppHandle;

use crate::belt::{self, BeltState};
use crate::panel::Rect;

#[tauri::command(async)]
pub fn belt_state(app: AppHandle) -> BeltState {
    belt::state_now(&app)
}

/// 页面开始监听事件后调用：补发一次当前状态
#[tauri::command(async)]
pub fn belt_ready(app: AppHandle) {
    belt::ready(&app);
}

/// 页面报能点的地方（窗口内逻辑像素），其余透明区域点击穿透
#[tauri::command(async)]
pub fn belt_hits(rects: Vec<Rect>) {
    belt::set_hits(rects);
}

/// 面板里放上几条记录，按给的顺序排在末尾
#[tauri::command(async)]
pub fn belt_put(app: AppHandle, ids: Vec<i64>) {
    belt::put(&app, &ids);
}

#[tauri::command(async)]
pub fn belt_pick(app: AppHandle, id: u64) {
    belt::pick(&app, id);
}

#[tauri::command(async)]
pub fn belt_reorder(app: AppHandle, ids: Vec<u64>) {
    belt::reorder(&app, &ids);
}

#[tauri::command(async)]
pub fn belt_remove(app: AppHandle, id: u64) {
    belt::remove(&app, id);
}

#[tauri::command(async)]
pub fn belt_undo_remove(app: AppHandle) {
    belt::undo_remove(&app);
}

#[tauri::command(async)]
pub fn belt_split(app: AppHandle, id: u64, accept: bool) {
    belt::split(&app, id, accept);
}

#[tauri::command(async)]
pub fn belt_back(app: AppHandle) {
    belt::back(&app);
}

#[tauri::command(async)]
pub fn belt_close(app: AppHandle) {
    belt::close_now(&app);
}

#[tauri::command(async)]
pub fn belt_undo_clear(app: AppHandle) {
    belt::undo_clear(&app);
}

/// 探出时点了“开始”
#[tauri::command(async)]
pub fn belt_start(app: AppHandle) {
    belt::begin_now(&app);
}
