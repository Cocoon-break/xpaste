use tauri::ipc::{InvokeBody, Request, Response};
use tauri::{AppHandle, WebviewWindow};

use crate::error::AppError;
use crate::{rec, shot};

fn index_of(window: &WebviewWindow) -> Result<usize, AppError> {
    shot::overlay_index(window.label()).ok_or_else(|| AppError::Invalid("不是截图遮罩".into()))
}

/// 调用方遮罩那块屏的冻结图：前 8 字节为小端 `u32` 宽、高，其后是 RGBA。每次截图每块屏只能取一次：
/// 响应体在冻结时已拼好，这里原样移交，不再拷贝。
#[tauri::command(async)]
pub fn shot_frame(app: AppHandle, window: WebviewWindow) -> Result<Response, AppError> {
    let i = index_of(&window)?;
    let bytes = shot::session(&app)
        .take_frame(i)
        .ok_or_else(|| AppError::Invalid("截图已结束".into()))?;
    Ok(Response::new(bytes))
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

/// 遮罩页心跳：已上屏的遮罩每秒调一次，后端 3 s 收不到就撤下遮罩（M23 规则 1）。
/// 返回这次截图是否仍在进行；`false` 时前端放掉像素、停止心跳。
/// 不走主线程：只量页面是否还活着。
#[tauri::command(async)]
pub fn shot_alive(app: AppHandle, generation: u64) -> bool {
    shot::alive(&app, generation)
}

/// 设置或确认框开着时按了截图或录屏快捷键、焦点不在快捷键录制框里：面板转回来开始（M15 交互规则 3、4）。
#[tauri::command]
pub fn overlay_begin(app: AppHandle, rec: bool) {
    let mode = if rec {
        shot::Mode::Rec
    } else {
        shot::Mode::Shot
    };
    shot::begin(&app, mode);
}

/// 退出截图：Esc / 右键，或提示条显示完后。隐藏全部遮罩、释放冻结图、撤销鼠标穿透。
/// `generation` 取自 `shot-begin` 载荷；不是当前这次截图时忽略（上一次截图迟到的退出不能结束新的一次）。
#[tauri::command]
pub fn shot_cancel(app: AppHandle, generation: u64) {
    shot::end_if_current(&app, generation);
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

/// 倒计时结束，开始录屏（M24）。`generation` 与 `index` 取自 `shot-begin` 载荷：只接受当前这一次截图、
/// 由该序号遮罩发起的调用。`rect` 为该屏内逻辑像素；两个声音开关与设置不同时写回设置。
#[tauri::command]
pub async fn rec_start(
    app: AppHandle,
    window: WebviewWindow,
    generation: u64,
    index: usize,
    rect: shot::LRect,
    system_audio: bool,
    microphone: bool,
) -> Result<(), AppError> {
    if index_of(&window)? != index {
        return Err(AppError::Invalid("显示器序号与遮罩不符".into()));
    }
    // 取屏幕、申请麦克风权限、开始录制都是阻塞调用
    tauri::async_runtime::spawn_blocking(move || {
        rec::start(&app, generation, index, rect, system_audio, microphone)
    })
    .await
    .map_err(|_| AppError::Invalid("录屏开始中断".into()))?
}

/// 录屏边框页注册好监听后拉一次当前位置（刚建的窗口可能错过推送）；不在录制时为 `null`。
#[tauri::command]
pub fn rec_frame(app: AppHandle) -> Option<shot::LRect> {
    rec::frame_rect(&app)
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

/// 调用方贴图换位置和尺寸（逻辑坐标）：滚轮缩放、恢复原始大小、拖出屏后拉回。
/// 返回尺寸实际乘上的系数（≤ 1，超过窗口尺寸上限时小于 1）。
#[tauri::command]
pub fn pin_set_bounds(
    window: WebviewWindow,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Result<f64, AppError> {
    shot::pin_set_bounds(&window, x, y, w, h)
}

/// 鼠标主键是否按着：贴图被系统拖动时，松开后才把拖出屏的拉回来。
#[tauri::command]
pub fn pin_button_down() -> bool {
    xpaste_platform::mouse_button_down()
}
