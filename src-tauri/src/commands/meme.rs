//! 表情包制作的命令：打开、关闭制作窗口，读参考图，风格库，设置页的“测试生成”与表情包库统计。

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::error::AppError;
use crate::imagegen::{self, Format, ImageCfg};
use crate::meme::{self, RefKey};
use crate::state::AppState;

/// 设置页表单里（未保存）的生图接口配置。带 Key 与地址，不派生 Debug。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageCfgDto {
    pub format: Format,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

/// “测试生成”的固定提示词：一只 Q 版猫。
const TEST_PROMPT: &str = "一只 Q 版的小猫，白色背景，简单可爱";

/// 用表单里的配置生成一张小图，返回 PNG；不存任何东西。失败返回给用户看的原因。
#[tauri::command]
pub async fn meme_test_image(cfg: ImageCfgDto) -> Result<tauri::ipc::Response, String> {
    let cfg = ImageCfg {
        format: cfg.format,
        base_url: cfg.base_url.trim().trim_end_matches('/').to_owned(),
        api_key: cfg.api_key.trim().to_owned(),
        model: cfg.model.trim().to_owned(),
    };
    if imagegen::unconfigured(&cfg) {
        return Err("接口地址和模型 ID 都要填".into());
    }
    let started = std::time::Instant::now();
    let r = imagegen::generate(&cfg, TEST_PROMPT, &[]).await;
    log::info!(
        "表情包测试生成 耗时 {} ms {}",
        started.elapsed().as_millis(),
        if r.is_ok() { "成功" } else { "失败" }
    );
    r.map(tauri::ipc::Response::new)
        .map_err(|e| imagegen::reason(&e))
}

/// 表情包库的个数与占用字节数（设置页底部“表情包库：N 个，占 X MB”）。
#[tauri::command(async)]
pub fn meme_library_stats(state: State<'_, AppState>) -> Result<(u64, u64), AppError> {
    Ok(state.store().meme_bytes()?)
}

/// 面板的笑脸按钮、`Mod+E`：带着这些记录打开（空 = 空白打开）。全都做不了表情包时返回原因。
#[tauri::command(async)]
pub fn meme_open(app: AppHandle, ids: Vec<i64>) -> Result<(), AppError> {
    meme::open(&app, &ids)
}

/// 页面的 Esc、关闭按钮
#[tauri::command(async)]
pub fn meme_close(app: AppHandle) {
    meme::close(&app);
}

/// 页面的参考图变了（拿掉、从历史添加、画草图）：后端据此判断再从面板带进来时还能加几张。
/// 草图不是历史记录，页面用负数 id 占位，照样算一张；再发 `meme-open` 时原样带回去。
#[tauri::command(async)]
pub fn meme_set_refs(refs: Vec<RefKey>) {
    meme::set_refs(refs);
}

/// 一张参考图的 PNG（动图取第一帧）。原记录没了、文件找不到时返回 "gone"，页面拿掉它并提示。
#[tauri::command(async)]
pub fn meme_ref_image(
    state: State<'_, AppState>,
    id: i64,
    file: Option<u32>,
) -> Result<tauri::ipc::Response, String> {
    crate::record_image::read_record_image(&state, id, file)
        .map(tauri::ipc::Response::new)
        .ok_or_else(|| "gone".into())
}

/// “从历史添加”：最近 8 张图片（图片文件记录按文件拆开）
#[tauri::command(async)]
pub fn meme_recent_images(state: State<'_, AppState>) -> Result<Vec<RefKey>, AppError> {
    meme::recent_images(&state)
}

/// 一种内置风格：键名、显示名、类别
#[derive(Serialize)]
pub struct StyleDto {
    key: &'static str,
    name: &'static str,
    cat: &'static str,
}

/// 风格库：内置风格，以及设置里挑过的、自己写的（页面据此画常驻芯片）。带用户写的风格描述，不派生 Debug。
#[derive(Serialize)]
pub struct StylesDto {
    styles: Vec<StyleDto>,
    used: Vec<String>,
    custom: Vec<String>,
}

#[tauri::command(async)]
pub fn meme_styles(state: State<'_, AppState>) -> StylesDto {
    let s = state.settings();
    StylesDto {
        styles: xpaste_pro::meme_styles()
            .iter()
            .map(|m| StyleDto {
                key: m.key,
                name: m.name,
                cat: m.cat,
            })
            .collect(),
        used: s.meme_styles_used.clone(),
        custom: s.meme_styles_custom.clone(),
    }
}

/// 挑过的风格、自己写的风格变了：整理（去重、限个数）后写回设置，下次打开还在
#[tauri::command(async)]
pub fn meme_styles_touch(
    app: AppHandle,
    state: State<'_, AppState>,
    used: Vec<String>,
    custom: Vec<String>,
) -> Result<(), String> {
    let mut s = state.settings().clone();
    (s.meme_styles_used, s.meme_styles_custom) = xpaste_pro::meme_normalize_styles(used, custom);
    crate::commands::settings::save_settings_inner(&state, s).map_err(|e| e.to_string())?;
    // 设置页开着时跟着刷新，免得它保存时把旧列表写回去；发的是存好之后的设置
    let saved = state.settings().clone();
    crate::events::emit_to(&app, "panel", crate::events::SETTINGS_CHANGED, saved);
    Ok(())
}
