use tauri::AppHandle;

use crate::error::AppError;

#[tauri::command(async)]
pub fn hide_panel(app: AppHandle) -> Result<(), AppError> {
    crate::panel::hide(&app);
    Ok(())
}

/// 系统设置中的辅助功能页（非 macOS 无操作）。AppKit 调用切到主线程。
#[tauri::command(async)]
pub fn open_accessibility_settings(app: AppHandle) -> Result<(), AppError> {
    app.run_on_main_thread(xpaste_platform::open_accessibility_settings)?;
    Ok(())
}
