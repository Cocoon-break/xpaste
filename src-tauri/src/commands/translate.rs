//! 翻译命令（设计 §4.5）。
//! 系统翻译与语言包查询靠主队列回调，不能在主线程上等；同步命令跑在主线程，所以这几个命令都是 async，
//! 平台调用经 `spawn_blocking`（见 `translate::system`）。

use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, Manager};
use xpaste_core::Settings;
use xpaste_platform::PackStatus;

use crate::error::AppError;
use crate::events::{TRANSLATE_DELTA, emit};
use crate::state::AppState;
use crate::translate::{self, Coalescer, Delta, Lang};

#[derive(Debug, Serialize)]
pub struct Translated {
    pub text: String,
    pub lang: String,
}

/// 翻译一条记录并存库；增量经 `translate-delta` 广播。同一条记录已在翻译时报“正在翻译”，前端忽略。
#[tauri::command]
pub async fn translate_record(app: AppHandle, id: i64) -> Result<Translated, AppError> {
    let state = app.state::<AppState>();
    let Some(text) = state.store().translate_source(id)? else {
        return Err(AppError::Invalid("这条记录不能翻译".into()));
    };
    if !state
        .translating
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(id)
    {
        return Err(AppError::Invalid("正在翻译".into()));
    }
    let cfg = translate::cfg(&state.settings());
    let t0 = Instant::now();
    let mut co = Coalescer::default();
    let mut lang = None;
    let send = |lang: Lang, text: String| {
        let p = Delta {
            id: Some(id),
            lang: lang.code().into(),
            text,
        };
        emit(&app, TRANSLATE_DELTA, p);
    };
    let r = translate::exec(&cfg, &text, None, |l, d| {
        lang = Some(l);
        if let Some(s) = co.push(d, Instant::now()) {
            send(l, s);
        }
        true
    })
    .await;
    if let (Some(l), Some(s)) = (lang, co.flush()) {
        send(l, s);
    }
    state
        .translating
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&id);
    let ms = t0.elapsed().as_millis();
    let (lang, out) = match r {
        Ok(v) => v,
        Err(f) => {
            log::info!("translate id={id} 失败 {} ms={ms}", f.tag());
            return Err(f.into());
        }
    };
    log::info!(
        "translate id={id} engine={} chars={} ms={ms} ok",
        cfg.engine.name(),
        text.chars().count()
    );
    state.store().set_translation(id, &out, lang.code())?;
    Ok(Translated {
        text: out,
        lang: lang.code().into(),
    })
}

/// 设置页“测试连接”：用表单里（未保存）的值翻译固定短句，不存任何东西（设计 §3.3）。
#[tauri::command]
pub async fn translate_test(
    engine: String,
    base_url: String,
    api_key: String,
    model: String,
    lang: String,
) -> Result<String, AppError> {
    if Lang::from_code(&lang).is_none() {
        return Err(AppError::Invalid("目标语言不对".into()));
    }
    let cfg = translate::cfg(&Settings {
        translate_engine: engine,
        translate_base_url: base_url,
        translate_api_key: api_key,
        translate_model: model,
        translate_lang: lang,
        ..Settings::default()
    });
    let (_, out) = translate::run(&cfg, "Hello, world.", |_, _| true).await?;
    Ok(out)
}

#[derive(Debug, Serialize)]
pub struct TranslateStatus {
    /// 本机能否用系统翻译
    pub supported: bool,
    /// 英语 ↔ 目标语言的语言包：`installed` | `download` | `unsupported`；不支持系统翻译时为 null
    pub pack: Option<&'static str>,
}

#[tauri::command]
pub async fn translate_status(lang: String) -> TranslateStatus {
    let supported = xpaste_platform::translate_supported();
    let pack = if supported {
        // 主队列回调，见文件头
        let s =
            tauri::async_runtime::spawn_blocking(move || xpaste_platform::translate_pack(&lang))
                .await
                .unwrap_or(PackStatus::Unsupported);
        Some(match s {
            PackStatus::Installed => "installed",
            PackStatus::Download => "download",
            PackStatus::Unsupported => "unsupported",
        })
    } else {
        None
    };
    TranslateStatus { supported, pack }
}

/// 打开系统设置“语言与地区”（翻译语言在这一页），打不开时退回打开系统设置。
#[tauri::command(async)]
pub fn open_translation_settings(app: AppHandle) {
    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_opener::OpenerExt;
        let o = app.opener();
        if o.open_url(
            "x-apple.systempreferences:com.apple.Localization-Settings.extension",
            None::<&str>,
        )
        .is_err()
            && let Err(e) = o.open_url("x-apple.systempreferences:", None::<&str>)
        {
            log::warn!("打开系统设置失败：{e}");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}
