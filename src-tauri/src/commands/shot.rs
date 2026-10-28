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
    // 只在锁里克隆 Arc，拷贝放锁外
    let f = shot::session(&app)
        .frame(i)
        .ok_or_else(|| AppError::Invalid("截图已结束".into()))?;
    Ok(Response::new(shot::frame_bytes(&f)))
}

/// 调用方遮罩本次截图的 `shot-begin` 载荷；没有进行中的截图或这块屏已上屏时为 `null`。
/// 推送的 `shot-begin` 可能早于前端注册好监听（刚建的遮罩），所以前端在 `await listen('shot-begin')`
/// 之后调一次，两路先到者生效。后端先存载荷再推送，监听注册之后再拉就不会两头落空。
#[tauri::command]
pub fn shot_begin_payload(
    app: AppHandle,
    window: WebviewWindow,
) -> Result<Option<shot::BeginPayload>, AppError> {
    Ok(shot::session(&app).payload(index_of(&window)?))
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
