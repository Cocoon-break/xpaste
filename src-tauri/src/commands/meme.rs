//! 表情包制作的命令：设置页的“测试生成”与表情包库统计。

use serde::Deserialize;
use tauri::State;

use crate::error::AppError;
use crate::imagegen::{self, Format, ImageCfg};
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
