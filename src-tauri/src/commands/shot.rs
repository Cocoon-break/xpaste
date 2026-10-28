use tauri::ipc::Response;
use tauri::{AppHandle, WebviewWindow};

use crate::error::AppError;
use crate::shot;

fn index_of(window: &WebviewWindow) -> Result<usize, AppError> {
    shot::overlay_index(window.label()).ok_or_else(|| AppError::Invalid("不是截图遮罩".into()))
}

/// 调用方遮罩那块屏的冻结图：前 8 字节为小端 `u32` 宽、高，其后是 RGBA。
/// 每屏几十 MB 的拷贝放异步线程，不占主线程。
#[tauri::command(async)]
pub fn shot_frame(app: AppHandle, window: WebviewWindow) -> Result<Response, AppError> {
    let i = index_of(&window)?;
    let s = shot::session(&app);
    let f = s
        .frame(i)
        .ok_or_else(|| AppError::Invalid("截图已结束".into()))?;
    Ok(Response::new(shot::frame_bytes(f)))
}

/// 遮罩已画好冻结图，可以上屏。
#[tauri::command]
pub fn shot_ready(app: AppHandle, window: WebviewWindow) -> Result<(), AppError> {
    shot::ready(&app, index_of(&window)?);
    Ok(())
}

#[tauri::command]
pub fn shot_cancel(app: AppHandle) {
    shot::end(&app);
}
