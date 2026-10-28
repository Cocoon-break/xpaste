use tauri::ipc::{InvokeBody, Request, Response};
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

/// 遮罩已画好冻结图，可以上屏。`generation` 取自 `shot-begin` 载荷；不是当前这次截图时忽略。
#[tauri::command]
pub fn shot_ready(app: AppHandle, window: WebviewWindow, generation: u64) -> Result<(), AppError> {
    shot::ready(&app, generation, index_of(&window)?);
    Ok(())
}

/// 退出截图：Esc / 右键，或提示条显示完后。隐藏全部遮罩、释放冻结图、撤销鼠标穿透。
#[tauri::command]
pub fn shot_cancel(app: AppHandle) {
    shot::end(&app);
}

/// 遮罩合成好的 PNG（请求体原始字节）。请求头：`x-action`（copy / save / ocr / pin）、
/// `x-index`（显示器序号，须与调用方遮罩一致）、`x-rect`（`x,y,w,h`，显示器内逻辑像素，贴图用）。
/// 成功后遮罩进入提示条模式（鼠标穿透、键盘已还给原应用），前端显示提示条后调 `shot_cancel`。
#[tauri::command]
pub async fn shot_finish(
    app: AppHandle,
    window: WebviewWindow,
    request: Request<'_>,
) -> Result<shot::ShotOutcome, AppError> {
    let invalid = |m: &str| AppError::Invalid(m.into());
    let header = |k| request.headers().get(k).and_then(|v| v.to_str().ok());
    let action = header("x-action")
        .and_then(shot::parse_action)
        .ok_or_else(|| invalid("未知的截图动作"))?;
    // 显示器序号以调用方遮罩为准，请求头只做核对
    let index = index_of(&window)?;
    if header("x-index").is_some_and(|v| v.parse() != Ok(index)) {
        return Err(invalid("显示器序号与遮罩不符"));
    }
    let rect = header("x-rect").and_then(shot::parse_rect);
    let InvokeBody::Raw(png) = request.body() else {
        return Err(invalid("请求体应为 PNG 字节"));
    };
    let png = png.clone();
    // 编码、写文件、识别文字都是阻塞调用
    tauri::async_runtime::spawn_blocking(move || shot::finish(&app, action, index, rect, png))
        .await
        .map_err(|_| invalid("截图处理中断"))?
}

/// 关闭调用方贴图（双击、Esc、右键“关闭”）。
#[tauri::command(async)]
pub fn pin_close(app: AppHandle, window: WebviewWindow) -> Result<(), AppError> {
    shot::pin_close(&app, &window)
}

/// 调用方贴图的原图写剪切板，进入历史。
#[tauri::command(async)]
pub fn pin_copy(app: AppHandle, window: WebviewWindow) -> Result<shot::ShotOutcome, AppError> {
    shot::pin_copy(&app, &window)
}

/// 保存调用方贴图的原图，规则同截图的保存。
#[tauri::command(async)]
pub fn pin_save(app: AppHandle, window: WebviewWindow) -> Result<shot::ShotOutcome, AppError> {
    shot::pin_save(&app, &window)
}
