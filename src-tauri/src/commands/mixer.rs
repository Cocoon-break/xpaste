//! 搅拌机页面与面板调用的命令。都在后台线程跑：要读库、读文件，不占主线程。

use tauri::AppHandle;

use crate::error::AppError;
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

/// 收成角标或展开
#[tauri::command(async)]
pub fn mixer_mini(app: AppHandle, on: bool) {
    mixer::mini(&app, on);
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

/// 补一句展开时拿键盘（`on`），收起时交还
#[tauri::command(async)]
pub fn mixer_keyboard(app: AppHandle, on: bool) {
    mixer::keyboard(&app, on);
}

/// 底座空白处按下：开始拖动换位置
#[tauri::command(async)]
pub fn mixer_drag() {
    crate::mixer_window::drag();
}

/// 设置页“恢复默认”用的自带配方
#[tauri::command(async)]
pub fn mixer_defaults() -> Vec<xpaste_core::model::Recipe> {
    xpaste_pro::default_recipes()
}

/// 按配方（None 为只按补一句）和补的那句开搅
#[tauri::command(async)]
pub fn mixer_blend(app: AppHandle, recipe: Option<String>, extra: String) -> Result<(), AppError> {
    mixer::blend(&app, recipe, extra)
}

#[tauri::command(async)]
pub fn mixer_stop(app: AppHandle) {
    mixer::stop(&app);
}

/// 重搅（None）或换配方（Some），沿用这次补的那句
#[tauri::command(async)]
pub fn mixer_reblend(app: AppHandle, recipe: Option<String>) -> Result<(), AppError> {
    mixer::reblend(&app, recipe)
}

/// 提示条上的“重试”：同一个配方和补的那句再来一次
#[tauri::command(async)]
pub fn mixer_retry(app: AppHandle) -> Result<(), AppError> {
    mixer::reblend(&app, None)
}

/// 倒掉结果，原料还在
#[tauri::command(async)]
pub fn mixer_dump(app: AppHandle) {
    mixer::dump(&app);
}

/// 把结果重新写进剪切板
#[tauri::command(async)]
pub fn mixer_put_back(app: AppHandle) {
    mixer::put_back(&app);
}

/// 贴出后的“再搅一次”
#[tauri::command(async)]
pub fn mixer_again(app: AppHandle) {
    mixer::again(&app);
}

/// 把补的那句存成配方，返回新名字
#[tauri::command(async)]
pub fn mixer_save_recipe(app: AppHandle, text: String) -> Result<String, AppError> {
    mixer::save_recipe(&app, &text)
}

/// 打开设置的搅拌机页；`edit` 给了时展开这个配方的编辑
#[tauri::command(async)]
pub fn mixer_settings(app: AppHandle, edit: Option<String>) {
    let tab = match edit {
        Some(name) => format!("mixer:{name}"),
        None => "mixer".into(),
    };
    crate::commands::panel::open_panel_settings(app, Some(tab));
}
