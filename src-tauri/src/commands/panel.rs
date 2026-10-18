use tauri::AppHandle;

use crate::error::AppError;

#[tauri::command(async)]
pub fn hide_panel(app: AppHandle) -> Result<(), AppError> {
    crate::panel::hide(&app);
    Ok(())
}
