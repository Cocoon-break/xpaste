//! 搅拌机页面与面板调用的命令。都在后台线程跑：要读库、读文件，不占主线程。

use tauri::AppHandle;

use crate::mixer::{self, MixerState};
use crate::panel::Rect;

#[tauri::command(async)]
pub fn mixer_state(app: AppHandle) -> MixerState {
    mixer::state_now(&app)
}

/// 页面开始监听事件后调用：补发一次当前状态
#[tauri::command(async)]
pub fn mixer_ready(app: AppHandle) {
    mixer::ready(&app);
}

/// 页面报能点的地方（窗口内逻辑像素），其余透明区域点击穿透
#[tauri::command(async)]
pub fn mixer_hits(rects: Vec<Rect>) {
    mixer::set_hits(rects);
}

/// 面板里放进几条记录，按给的顺序排在最后
#[tauri::command(async)]
pub fn mixer_put(app: AppHandle, ids: Vec<i64>) {
    mixer::put(&app, &ids);
}

#[tauri::command(async)]
pub fn mixer_remove(app: AppHandle, id: u64) {
    mixer::remove(&app, id);
}

#[tauri::command(async)]
pub fn mixer_undo_remove(app: AppHandle) {
    mixer::undo_remove(&app);
}

#[tauri::command(async)]
pub fn mixer_close(app: AppHandle) {
    mixer::close_now(&app);
}

#[tauri::command(async)]
pub fn mixer_undo_clear(app: AppHandle) {
    mixer::undo_clear(&app);
}

/// 页面播完收起、提示条也消失了
#[tauri::command(async)]
pub fn mixer_release(app: AppHandle) {
    mixer::release(&app);
}
