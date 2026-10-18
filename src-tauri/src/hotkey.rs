use tauri::AppHandle;
use tauri_plugin_global_shortcut::GlobalShortcutExt;

use crate::error::AppError;

/// 注销旧快捷键后注册新的；任何失败都视为被占用。
pub fn register(app: &AppHandle, shortcut: &str) -> Result<(), AppError> {
    let gs = app.global_shortcut();
    gs.unregister_all()
        .and_then(|()| gs.register(shortcut))
        .map_err(|e| {
            log::debug!("全局快捷键插件错误：{e}");
            AppError::HotkeyTaken
        })
}
