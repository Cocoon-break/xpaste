//! 翻译命令（M26）。
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
use crate::translate::{self, Cfg, Coalescer, Delta, Lang, Timeouts};

#[derive(Debug, Serialize)]
pub struct Translated {
    pub text: String,
    pub lang: String,
}

/// 翻译一条记录并存库；增量经 `translate-delta` 广播。同一条记录已在翻译时报“正在翻译”，前端忽略。
/// `again` 为假且已有存库译文时直接返回它，不发请求；只有“重新翻译”传真（M26 规则 4）。
#[tauri::command]
pub async fn translate_record(
    app: AppHandle,
    id: i64,
    again: bool,
) -> Result<Translated, AppError> {
    let state = app.state::<AppState>();
    let cfg = translate::cfg(&state.settings());
    translate_one(&state, &cfg, None, id, again, |p| {
        emit(&app, TRANSLATE_DELTA, p)
    })
    .await
}

/// `translate_record` 的实现，测试传短超时与自己的增量接收。
async fn translate_one(
    state: &AppState,
    cfg: &Cfg,
    timeouts: Option<Timeouts>,
    id: i64,
    again: bool,
    send: impl Fn(Delta) + Sync,
) -> Result<Translated, AppError> {
    let Some(text) = state.store().translate_source(id)? else {
        return Err(AppError::Invalid("这条记录不能翻译".into()));
    };
    if !again
        && let Some(d) = state.store().detail(id)?
        && let Some(t) = d.translation
    {
        return Ok(Translated {
            text: t,
            lang: d.translation_lang.unwrap_or_default(),
        });
    }
    if !state
        .translating
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(id)
    {
        return Err(AppError::Invalid("正在翻译".into()));
    }
    let t0 = Instant::now();
    let mut co = Coalescer::default();
    let mut lang = None;
    let send = |lang: Lang, text: String| {
        send(Delta {
            id: Some(id),
            lang: lang.code().into(),
            text,
        });
    };
    let r = translate::exec(cfg, &text, timeouts, |l, d| {
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
    // 翻译期间片段被编辑或识别结果变了（已清掉译文）：旧原文的译文不能存（M26 规则 12）。查与写在同一把锁里
    let store = state.store();
    if store.translate_source(id)?.as_deref() != Some(text.as_str()) {
        log::info!("translate id={id} 原文已改动，不存 ms={ms}");
        return Err(AppError::Translate("原文已改动，请重新翻译".into()));
    }
    log::info!(
        "translate id={id} engine={} chars={} ms={ms} ok",
        cfg.engine.name(),
        text.chars().count()
    );
    store.set_translation(id, &out, lang.code())?;
    Ok(Translated {
        text: out,
        lang: lang.code().into(),
    })
}

/// 设置页“测试翻译”：用表单里（未保存）的值翻译固定短句，不存任何东西（M26 规则 6）。
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::translate::tests::{FAST, SSE_HEAD, delta, openai, serve};
    use std::net::TcpListener;

    fn run(
        state: &AppState,
        cfg: &Cfg,
        id: i64,
        again: bool,
        on: impl Fn(Delta) + Sync,
    ) -> Result<Translated, AppError> {
        tauri::async_runtime::block_on(translate_one(state, cfg, Some(FAST), id, again, on))
    }

    /// 已有译文时不是“重新翻译”就直接返回存库译文，不发请求（M26 规则 4）。
    #[test]
    fn stored_translation_is_returned_without_request() {
        let state = AppState::for_test();
        let id = state.store().create_snippet("n", "hello", 1).unwrap();
        state
            .store()
            .set_translation(id, "你好", "zh-Hans")
            .unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let cfg = openai(&format!(
            "http://127.0.0.1:{}/v1",
            l.local_addr().unwrap().port()
        ));
        let t = run(&state, &cfg, id, false, |_| panic!("不应有增量")).unwrap();
        assert_eq!((t.text.as_str(), t.lang.as_str()), ("你好", "zh-Hans"));
        assert!(l.accept().is_err(), "已有译文不应发请求");

        // 重新翻译照常请求并覆盖
        let body = format!("{SSE_HEAD}{}data: [DONE]\n\n", delta("嗨"));
        let (url, _h) = serve(vec![(0, body.into_bytes())], false);
        let t = run(&state, &openai(&url), id, true, |_| {}).unwrap();
        assert_eq!(t.text, "嗨");
        let d = state.store().detail(id).unwrap().unwrap();
        assert_eq!(d.translation.as_deref(), Some("嗨"));
    }

    /// 翻译期间原文被改（片段编辑清掉了译文）：旧原文的译文不存，报“原文已改动”（M26 规则 12）。
    #[test]
    fn changed_source_mid_flight_is_not_stored() {
        let state = AppState::for_test();
        let id = state.store().create_snippet("n", "hello", 1).unwrap();
        let body = format!("{SSE_HEAD}{}data: [DONE]\n\n", delta("你好"));
        let (url, _h) = serve(vec![(0, body.into_bytes())], false);
        // 增量到达时改片段内容，模拟请求进行中用户编辑了片段
        let e = run(&state, &openai(&url), id, false, |_| {
            state.store().update_snippet(id, "n", Some("bye")).unwrap();
        })
        .unwrap_err();
        assert_eq!(e.kind(), "translate");
        assert_eq!(e.to_string(), "原文已改动，请重新翻译");
        assert_eq!(state.store().detail(id).unwrap().unwrap().translation, None);
        assert!(state.translating.lock().unwrap().is_empty());
    }
}
