//! 表情包制作的命令：打开、关闭制作窗口，读参考图，风格库，出文案、生成、抠图贴纸、存草图，设置页的“测试生成”与表情包库统计。

use serde::{Deserialize, Serialize};
use tauri::ipc::{InvokeBody, Request};
use tauri::{AppHandle, State};

use crate::error::AppError;
use crate::imagegen::{self, Format, ImageCfg};
use crate::meme::{self, RefKey};
use crate::meme_gen::{self, GenOut, GenReq};
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

/// 页面的“存进表情包”存好之后：关窗口，面板切到“表情包”分类选中它
#[tauri::command(async)]
pub fn meme_reveal(app: AppHandle, id: i64) {
    meme::reveal(&app, id);
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
    prefs: PrefsDto,
}

/// 上次的选择：风格、文字样式、文字位置、“更多调整”是否展开
#[derive(Serialize)]
pub struct PrefsDto {
    style: String,
    ts: String,
    pos: String,
    more: bool,
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
        prefs: PrefsDto {
            style: s.meme_last_style.clone(),
            ts: s.meme_last_text_style.clone(),
            pos: s.meme_last_text_pos.clone(),
            more: s.meme_more_open,
        },
    }
}

/// 没有参考图时，按文字挑内置角色的表情键名
#[tauri::command]
pub fn meme_base_emotion(text: String) -> String {
    xpaste_pro::meme_base_emotion(&text).to_string()
}

/// 记住这次的风格、文字样式、位置、“更多调整”是否展开，下次打开默认用上
#[tauri::command(async)]
pub fn meme_prefs_set(
    app: AppHandle,
    state: State<'_, AppState>,
    style: String,
    ts: String,
    pos: String,
    more: bool,
) -> Result<(), String> {
    let mut s = state.settings().clone();
    s.set_meme_prefs(&style, &ts, &pos, more);
    crate::commands::settings::save_settings_inner(&state, s).map_err(|e| e.to_string())?;
    let saved = state.settings().clone();
    crate::events::emit_to(&app, "panel", crate::events::SETTINGS_CHANGED, saved);
    Ok(())
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

/// 出文案：用翻译里配置的 AI 接口给 5 条候选。`sketch` 是第一张为草图时它的文件名；`avoid` 是“换一批”前给过的
#[tauri::command]
pub async fn meme_captions(
    state: State<'_, AppState>,
    idea: String,
    refs: Vec<RefKey>,
    sketch: Option<String>,
    avoid: Vec<String>,
) -> Result<Vec<String>, String> {
    let cfg = crate::translate::cfg(&state.settings());
    meme_gen::captions(&state, &cfg, &idea, &refs, sketch.as_deref(), &avoid).await
}

/// 生成一张 / 一套 / 一套里的一部分 / AI 动作，结果立刻进库。取消或关窗口后才回来的，返回 `meme_gen::LATE`
#[tauri::command]
pub async fn meme_generate(
    app: AppHandle,
    state: State<'_, AppState>,
    req: GenReq,
) -> Result<GenOut, String> {
    let cfg = meme_gen::image_cfg(&state.settings());
    let added = |id| crate::events::emit_summary(&app, crate::events::RECORD_ADDED, id);
    let env = meme_gen::Env {
        state: &state,
        seq: &meme_gen::SEQ,
        cutout: xpaste_platform::foreground_cutout,
        added: &added,
    };
    meme_gen::generate(&env, &cfg, req).await
}

/// 取消生成：请求不打断，回来的结果照样进库，只是不交给页面
#[tauri::command(async)]
pub fn meme_cancel() {
    meme_gen::cancel();
    log::info!("表情包 取消生成");
}

/// 发起一个 AI 任务（排队，不跟窗口走），返回任务号。`title` 是列表里显示的那句话，`scene` 是发起时的做法 JSON，都原样带回
#[tauri::command(async)]
pub fn meme_task_start(app: AppHandle, req: GenReq, scene: String, title: String) -> u64 {
    crate::meme_tasks::start(&app, req, title, scene)
}

/// 取消任务：排队的直接拿掉；在画的请求不中止，结果回来照样进库，不再显示
#[tauri::command(async)]
pub fn meme_task_cancel(app: AppHandle, id: u64) {
    crate::meme_tasks::cancel(&app, id);
}

/// 没画出来的重试：回到队尾
#[tauri::command(async)]
pub fn meme_task_retry(app: AppHandle, id: u64) {
    crate::meme_tasks::retry(&app, id);
}

/// 去掉画好的、没画出来的
#[tauri::command(async)]
pub fn meme_task_drop(app: AppHandle, id: u64) {
    crate::meme_tasks::drop(&app, id);
}

/// 回到那次：按任务里的做法打开制作窗口，结果摆进预览；画好的随之拿掉
#[tauri::command(async)]
pub fn meme_task_open(app: AppHandle, id: u64) -> Result<(), AppError> {
    crate::meme_tasks::open(&app, id)
}

/// 整份任务列表，新的在前
#[tauri::command(async)]
pub fn meme_tasks() -> Vec<crate::meme_tasks::TaskView> {
    crate::meme_tasks::views()
}

/// 页面要的生图接口状态：配好了没有，这个服务商上次的一套画规整了没有
#[derive(Serialize)]
pub struct GenInfo {
    ready: bool,
    shaky: bool,
}

#[tauri::command(async)]
pub fn meme_gen_info(state: State<'_, AppState>) -> GenInfo {
    let cfg = meme_gen::image_cfg(&state.settings());
    GenInfo {
        ready: !imagegen::unconfigured(&cfg),
        shaky: meme_gen::grid_shaky(&cfg),
    }
}

/// 抠图贴纸：请求体是图片的 PNG，返回抠出主体加白边的 PNG。没抠出主体时返回原因
#[tauri::command]
pub async fn meme_cutout(request: Request<'_>) -> Result<tauri::ipc::Response, String> {
    let InvokeBody::Raw(png) = request.body() else {
        return Err("请求体应为 PNG 字节".into());
    };
    let png = png.clone();
    let started = std::time::Instant::now();
    // 系统抠主体是阻塞调用
    let out = tauri::async_runtime::spawn_blocking(move || {
        meme_gen::sticker(xpaste_platform::foreground_cutout, &png)
    })
    .await
    .ok()
    .flatten();
    log::info!(
        "表情包 抠图贴纸 {} 耗时 {} ms",
        if out.is_some() {
            "成功"
        } else {
            "没抠出主体"
        },
        started.elapsed().as_millis()
    );
    out.map(tauri::ipc::Response::new)
        .ok_or_else(|| "没找到能抠出来的主体".into())
}

/// 存草图：请求体是 PNG，存到 `memes/sketch/`，返回文件名
#[tauri::command]
pub async fn meme_save_sketch(
    state: State<'_, AppState>,
    request: Request<'_>,
) -> Result<String, String> {
    let InvokeBody::Raw(png) = request.body() else {
        return Err("请求体应为 PNG 字节".into());
    };
    meme_gen::save_sketch(&state.data_dir, png)
}

/// 库里一个表情包的第 `n` 帧 PNG（AI 动作的 GIF 给预览用；不是动图时只有第 0 帧）。读不到时返回 "gone"
#[tauri::command(async)]
pub fn meme_frame(
    state: State<'_, AppState>,
    id: i64,
    n: usize,
) -> Result<tauri::ipc::Response, String> {
    meme_gen::frame_png(&state, id, n)
        .map(tauri::ipc::Response::new)
        .ok_or_else(|| "gone".into())
}

/// 库里这几个表情包的名字（一套 16 张时是各格的情绪名）；读不到的为空串
#[tauri::command(async)]
pub fn meme_names(state: State<'_, AppState>, ids: Vec<i64>) -> Vec<String> {
    meme_gen::names(&state, &ids)
}

/// 导出的一级：请求体是 JSON 帧头 {side, delayMs, colors, count} + `\n` + 各帧 RGBA。一帧编成 PNG，多帧编成 GIF；
/// 只在内存里留最近一次的结果，返回它的序号和字节数（页面据此决定要不要再降一级）
#[tauri::command]
pub async fn meme_encode(request: Request<'_>) -> Result<crate::meme_store::EncodeOut, String> {
    let InvokeBody::Raw(body) = request.body() else {
        return Err("请求体应为帧数据".into());
    };
    let body = body.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::meme_store::encode(&crate::meme_store::LAST, &body)
    })
    .await
    .map_err(|_| "编码失败".to_string())?
}

/// `meme_store` 的结果：库里的 id；放剪切板失败时带提示（这时已进库）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreOut {
    id: i64,
    clip_error: Option<&'static str>,
}

/// 存进表情包库（做好的），`copy` 时同时放进剪切板（自身写入，不进历史）
#[tauri::command(async)]
pub fn meme_store(
    app: AppHandle,
    state: State<'_, AppState>,
    token: u64,
    name: String,
    scene: String,
    search_parts: crate::meme_store::SearchParts,
    copy: bool,
) -> Result<StoreOut, String> {
    let f = crate::meme_store::Final {
        token,
        name,
        scene,
        parts: search_parts,
    };
    let mut write = |f: &[xpaste_core::Flavor]| xpaste_platform::write(f);
    let (id, clip_error) = crate::meme_store::store(
        &state,
        &crate::meme_store::LAST,
        f,
        copy.then_some(&mut write as crate::meme_store::Write<'_>),
    )?;
    // 进了库就让面板刷新（复制失败时也已进库）
    crate::events::emit_summary(&app, crate::events::RECORD_ADDED, id);
    Ok(StoreOut { id, clip_error })
}

/// 和上次存的一样：把库里那一个放进剪切板（自身写入，不进历史）。不在了时返回 "gone"
#[tauri::command(async)]
pub fn meme_copy(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    crate::meme_store::copy(&state, id, &mut |f| xpaste_platform::write(f))
}

/// 接着改：按库里这个表情包的做法打开制作窗口
#[tauri::command(async)]
pub fn meme_open_scene(app: AppHandle, id: i64) -> Result<(), AppError> {
    meme::open_scene(&app, id)
}

/// 关窗口时留草稿（替换旧的）；None 时丢掉
#[tauri::command(async)]
pub fn meme_draft_set(state: State<'_, AppState>, scene: Option<String>) {
    crate::meme_store::draft_set(&state.data_dir, scene.as_deref());
}

/// 做法里一张草图的 PNG（`memes/sketch/` 下的文件名）。读不到时返回 "gone"
#[tauri::command(async)]
pub fn meme_sketch_image(
    state: State<'_, AppState>,
    name: String,
) -> Result<tauri::ipc::Response, String> {
    meme_gen::sketch_path(&state.data_dir, &name)
        .and_then(|p| std::fs::read(p).ok())
        .map(tauri::ipc::Response::new)
        .ok_or_else(|| "gone".into())
}
