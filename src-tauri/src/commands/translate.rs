//! 翻译命令（M26）。
//! 系统翻译与语言包查询靠主队列回调，不能在主线程上等；同步命令跑在主线程，所以这几个命令都是 async，
//! 平台调用经 `spawn_blocking`（见 `translate::system`）。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, Manager};
use xpaste_core::Settings;
use xpaste_platform::PackStatus;

use crate::error::AppError;
use crate::events::{TRANSLATE_DELTA, emit};
use crate::state::AppState;
use crate::translate::{self, Cfg, Coalescer, Delta, Engine, Fail, Lang, Timeouts};

#[derive(Debug, Serialize)]
pub struct Translated {
    pub text: String,
    pub lang: String,
    /// 译文用的翻译方式标签（`system` / `openai:{模型}`）
    pub engine: String,
}

/// 翻译一条记录并存库；增量经 `translate-delta` 广播。同一条记录已在翻译时报“正在翻译”，前端忽略；
/// 指定了 `engine`（换用）时改为接替：旧请求中止、报“已换用”，前端忽略。
/// `again` 为假、没指定方式且已有存库译文时直接返回它，不发请求；只有“重新翻译”传真（M26 规则 4）。
/// `engine`（`system` / `openai`）单次指定翻译方式，不改设置；没指定时“重新翻译”沿用库里译文的方式（M26 规则 16）。
#[tauri::command]
pub async fn translate_record(
    app: AppHandle,
    id: i64,
    again: bool,
    engine: Option<String>,
) -> Result<Translated, AppError> {
    let engine = parse_engine(engine.as_deref())?;
    let state = app.state::<AppState>();
    let cfg = translate::cfg_for(&state.settings(), engine);
    translate_one(&state, &cfg, None, id, again, engine, |p| {
        emit(&app, TRANSLATE_DELTA, p)
    })
    .await
}

/// 前端单次指定的翻译方式（`system` / `openai`），没指定为 None（M26 规则 16）。
pub(crate) fn parse_engine(name: Option<&str>) -> Result<Option<Engine>, AppError> {
    match name {
        None => Ok(None),
        Some("system") => Ok(Some(Engine::System)),
        Some("openai") => Ok(Some(Engine::OpenAi)),
        Some(_) => Err(AppError::Invalid("翻译方式不对".into())),
    }
}

/// `translate_record` 的实现，测试传短超时与自己的增量接收。`cfg` 是按设置（及 `engine`）算出的配置。
/// 选翻译方式：`engine` 给了就用它；否则“重新翻译”沿用库里译文的方式；其余用设置。指定的方式用不了就报错，不换另一种。
async fn translate_one(
    state: &AppState,
    cfg: &Cfg,
    timeouts: Option<Timeouts>,
    id: i64,
    again: bool,
    engine: Option<Engine>,
    send: impl Fn(Delta) + Sync,
) -> Result<Translated, AppError> {
    let Some(text) = state.store().translate_source(id)? else {
        return Err(AppError::Invalid("这条记录不能翻译".into()));
    };
    let stored = state.store().detail(id)?;
    if !again
        && engine.is_none()
        && let Some(d) = &stored
        && let Some(t) = &d.translation
    {
        return Ok(Translated {
            text: t.clone(),
            lang: d.translation_lang.clone().unwrap_or_default(),
            engine: d.translation_engine.clone().unwrap_or_default(),
        });
    }
    let want = engine.or_else(|| {
        stored
            .as_ref()
            .filter(|_| again)
            .and_then(|d| d.translation_engine.as_deref())
            .and_then(Engine::from_tag)
    });
    let cfg = &match want {
        Some(e) if e != cfg.engine => Cfg {
            engine: e,
            ..cfg.clone()
        },
        _ => cfg.clone(),
    };
    let tag = cfg.tag();
    // 请求序号：翻译中“换用…”（指定了方式）接替进行中的请求，旧请求见序号变了就中止（M26 规则 16）
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    {
        let mut m = state
            .translating
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if engine.is_none() && m.contains_key(&id) {
            return Err(AppError::Invalid("正在翻译".into()));
        }
        m.insert(id, seq);
    }
    let live = || {
        state
            .translating
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&id)
            == Some(&seq)
    };
    let t0 = Instant::now();
    let mut co = Coalescer::default();
    let mut lang = None;
    let send = |lang: Lang, text: String| {
        send(Delta {
            id: Some(id),
            lang: lang.code().into(),
            engine: tag.clone(),
            text,
        });
    };
    let r = translate::exec(cfg, &text, timeouts, |l, d| {
        if !live() {
            return false;
        }
        lang = Some(l);
        if let Some(s) = co.push(d, Instant::now()) {
            send(l, s);
        }
        true
    })
    .await;
    let ms = t0.elapsed().as_millis();
    {
        let mut m = state
            .translating
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if m.get(&id) != Some(&seq) {
            log::info!("translate id={id} 已换用 ms={ms}");
            return Err(AppError::Invalid("已换用".into()));
        }
        m.remove(&id);
    }
    if let (Some(l), Some(s)) = (lang, co.flush()) {
        send(l, s);
    }
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
    store.set_translation(id, &out, lang.code(), &tag)?;
    Ok(Translated {
        text: out,
        lang: lang.code().into(),
        engine: tag,
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
pub struct Engines {
    /// 本机能否用系统翻译
    pub system: bool,
    /// AI 翻译已配置（接口地址、模型 ID 都不为空）
    pub openai: bool,
    /// 去掉首尾空白的模型 ID
    pub model: String,
    /// 按设置会用的翻译方式标签（`Cfg::tag`）：截图翻译出第一段之前就失败时，卡片标题行据此标出方式
    pub current: String,
}

/// 两种翻译方式各自能不能用，给“换用”按钮判断（M26 规则 16）。
#[tauri::command]
pub async fn translate_engines(app: AppHandle) -> Engines {
    let state = app.state::<AppState>();
    let s = state.settings();
    Engines {
        system: xpaste_platform::translate_supported(),
        openai: !s.translate_base_url.trim().is_empty() && !s.translate_model.trim().is_empty(),
        model: s.translate_model.trim().to_string(),
        current: translate::cfg(&s).tag(),
    }
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

/// 下载语言包（M26 规则 17）：在 `window_label` 那个正在显示的窗口里弹出系统的下载确认，等下载结束，
/// 返回 `installed` 或 `download`（取消了）。`source` 为空串时用设置那一行的语言对（英语 ↔ 目标，目标是英语时为简体中文）。
/// 确认框弹不出来时退回打开“语言与地区”，报错里说明去哪下载。
#[tauri::command]
pub async fn translate_download(
    app: AppHandle,
    window_label: String,
    source: String,
    target: String,
) -> Result<&'static str, AppError> {
    let Some(to) = Lang::from_code(&target) else {
        return Err(AppError::Invalid("目标语言不对".into()));
    };
    let source = match source.as_str() {
        "" if to == Lang::En => "zh-Hans".to_string(),
        "" => "en".to_string(),
        _ => source,
    };
    let w = app
        .get_webview_window(&window_label)
        .ok_or_else(|| AppError::Invalid("窗口不存在".into()))?;
    // ns_window 经主线程取指针（Tauri 内部转到主线程）；视图由 Swift 侧派到主队列挂上，这里只在阻塞线程上等（见文件头）
    #[cfg(target_os = "macos")]
    let ns = w.ns_window()? as usize;
    #[cfg(not(target_os = "macos"))]
    let ns = {
        let _ = w;
        0usize
    };
    let state = app.state::<AppState>();
    let Some(guard) = Downloading::begin(&state.tr_downloading) else {
        // 前端静默忽略
        return Err(AppError::Invalid("正在下载语言包".into()));
    };
    let t0 = Instant::now();
    let src = source.clone();
    let r = tauri::async_runtime::spawn_blocking(move || {
        // SAFETY: 面板（及其他 xPaste 窗口）常驻、不销毁，指针在等待期间一直有效
        unsafe { xpaste_platform::translate_prepare(&src, &target, ns as *mut std::ffi::c_void) }
    })
    .await;
    drop(guard);
    // 期间被忽略的失焦补一次隐藏，同 set_dialog_open；设置还开着时不隐藏
    if !state.dialog_open.load(Ordering::SeqCst) {
        crate::panel::hide_if_unfocused(&app);
    }
    let r = r?;
    let ms = t0.elapsed().as_millis();
    use xpaste_platform::SysTranslateError as E;
    match r {
        Ok(PackStatus::Installed) => {
            log::info!("trpack 下载 installed ms={ms}");
            Ok("installed")
        }
        Ok(_) => {
            log::info!("trpack 下载 未完成 ms={ms}");
            Ok("download")
        }
        Err(E::OsTooOld) => Err(AppError::TranslateOs),
        Err(E::Failed) => {
            log::warn!("trpack 确认框弹不出，打开语言与地区 ms={ms}");
            open_translation_settings(app.clone());
            Err(AppError::Translate(format!(
                "在这一页最下方点“翻译语言…”，下载{}和{}",
                translate::lang_name(&source),
                to.label()
            )))
        }
        Err(_) => Err(Fail::Unsupported.into()),
    }
}

/// 下载语言包进行中的标记（`AppState::tr_downloading`）：拿到才下载，drop 时复位，提前返回也不会卡住。
struct Downloading<'a>(&'a AtomicBool);

impl<'a> Downloading<'a> {
    /// 已有一个在下载时为 None
    fn begin(flag: &'a AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self(flag))
    }
}

impl Drop for Downloading<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
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
        run_with(state, cfg, id, again, None, on)
    }

    fn run_with(
        state: &AppState,
        cfg: &Cfg,
        id: i64,
        again: bool,
        engine: Option<Engine>,
        on: impl Fn(Delta) + Sync,
    ) -> Result<Translated, AppError> {
        tauri::async_runtime::block_on(translate_one(state, cfg, Some(FAST), id, again, engine, on))
    }

    /// 已有译文时不是“重新翻译”就直接返回存库译文，不发请求（M26 规则 4）。
    #[test]
    fn stored_translation_is_returned_without_request() {
        let state = AppState::for_test();
        let id = state.store().create_snippet("n", "hello", 1).unwrap();
        state
            .store()
            .set_translation(id, "你好", "zh-Hans", "openai:m")
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

    /// 重新翻译沿用库里译文的方式；不重新翻译时直接返回存的（M26 规则 16）。
    #[test]
    fn engine_follows_stored_translation() {
        let state = AppState::for_test();
        let id = state.store().create_snippet("n", "hello", 1).unwrap();
        state
            .store()
            .set_translation(id, "你好", "zh-Hans", "openai:m1")
            .unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let mut sys = openai(&format!(
            "http://127.0.0.1:{}/v1",
            l.local_addr().unwrap().port()
        ));
        sys.model = "m1".into();
        sys.engine = Engine::System; // 设置选的是系统翻译
        let t = run(&state, &sys, id, false, |_| panic!("不应有增量")).unwrap();
        assert_eq!(t.engine, "openai:m1");
        assert!(l.accept().is_err(), "不重新翻译不应发请求");

        let body = format!("{SSE_HEAD}{}data: [DONE]\n\n", delta("嗨"));
        let (url, _h) = serve(vec![(0, body.into_bytes())], false);
        let mut c = openai(&url);
        c.model = "m1".into();
        c.engine = Engine::System;
        let t = run(&state, &c, id, true, |_| {}).unwrap();
        assert_eq!((t.text.as_str(), t.engine.as_str()), ("嗨", "openai:m1"));
        let d = state.store().detail(id).unwrap().unwrap();
        assert_eq!(d.translation_engine.as_deref(), Some("openai:m1"));
    }

    /// 单次指定方式：照指定的翻、标注、不碰设置；已有译文也重新翻译。
    #[test]
    fn explicit_engine_does_not_change_settings() {
        let state = AppState::for_test();
        let id = state.store().create_snippet("n", "hello", 1).unwrap();
        state
            .store()
            .set_translation(id, "旧", "zh-Hans", "system")
            .unwrap();
        let before = state.settings().translate_engine.clone();
        let body = format!("{SSE_HEAD}{}data: [DONE]\n\n", delta("嗨"));
        let (url, _h) = serve(vec![(0, body.into_bytes())], false);
        let mut c = openai(&url);
        c.engine = Engine::System;
        let t = run_with(&state, &c, id, false, Some(Engine::OpenAi), |_| {}).unwrap();
        assert_eq!((t.text.as_str(), t.engine.as_str()), ("嗨", "openai:m"));
        assert_eq!(state.settings().translate_engine, before);
        let d = state.store().detail(id).unwrap().unwrap();
        assert_eq!(d.translation_engine.as_deref(), Some("openai:m"));
    }

    /// 指定的方式用不了：报对应错误，不改用另一种，不发请求。
    #[test]
    fn explicit_unavailable_engine_errors() {
        let state = AppState::for_test();
        let id = state.store().create_snippet("n", "hello", 1).unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let mut c = openai(&format!(
            "http://127.0.0.1:{}/v1",
            l.local_addr().unwrap().port()
        ));
        c.model = String::new();
        let e = run_with(&state, &c, id, false, Some(Engine::OpenAi), |_| {}).unwrap_err();
        assert_eq!(e.kind(), "translateConfig");
        assert!(l.accept().is_err());
    }

    /// 同一时刻只下载一个语言包；结束（含提前返回）后复位，可以再下载（M26 规则 17）。
    #[test]
    fn download_flag_allows_one_at_a_time() {
        let state = AppState::for_test();
        let g = Downloading::begin(&state.tr_downloading).unwrap();
        assert!(
            state.tr_downloading.load(Ordering::SeqCst),
            "下载期间失焦不隐藏"
        );
        assert!(Downloading::begin(&state.tr_downloading).is_none());
        drop(g);
        assert!(!state.tr_downloading.load(Ordering::SeqCst));
        assert!(Downloading::begin(&state.tr_downloading).is_some());
    }

    /// 翻译中换用（M26 规则 16）：同一条在翻译时，没指定方式的请求报“正在翻译”，指定了方式的接替；
    /// 被接替的请求不存库、报“已换用”，也不清掉接替者的登记。
    #[test]
    fn switch_takes_over_in_flight() {
        let state = AppState::for_test();
        let id = state.store().create_snippet("n", "hello", 1).unwrap();
        state.translating.lock().unwrap().insert(id, 0);
        let e = run(&state, &openai("http://127.0.0.1:9/v1"), id, true, |_| {}).unwrap_err();
        assert_eq!(e.to_string(), "正在翻译");
        let body = format!("{SSE_HEAD}{}data: [DONE]\n\n", delta("你好"));
        let (url, _h) = serve(vec![(0, body.into_bytes())], false);
        let t = run_with(
            &state,
            &openai(&url),
            id,
            true,
            Some(Engine::OpenAi),
            |_| {},
        )
        .unwrap();
        assert_eq!(t.text, "你好");
        assert!(state.translating.lock().unwrap().is_empty());

        // 请求进行中被换用：增量到达时登记换成了别的序号
        let first = format!("{SSE_HEAD}{}", delta("旧"));
        let rest = format!("{}data: [DONE]\n\n", delta("的"));
        // 隔 80 ms 再发第二段，合并器才在请求进行中吐出增量
        let (url, _h) = serve(
            vec![(0, first.into_bytes()), (80, rest.into_bytes())],
            false,
        );
        let e = run(&state, &openai(&url), id, true, |_| {
            state.translating.lock().unwrap().insert(id, u64::MAX);
        })
        .unwrap_err();
        assert_eq!(e.to_string(), "已换用");
        let d = state.store().detail(id).unwrap().unwrap();
        assert_eq!(d.translation.as_deref(), Some("你好"), "被接替的不存库");
        assert_eq!(state.translating.lock().unwrap().get(&id), Some(&u64::MAX));
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

/// 设置或确认框开着时按了翻译快捷键、焦点不在快捷键录制框里：面板转回来照常弹划词翻译卡片（M26 规则 18、M15 交互规则 3）。
/// 同步命令，在主线程上跑，同 `overlay_begin`。
#[tauri::command]
pub fn trcard_press(app: AppHandle) {
    crate::trcard::pressed(&app);
}

/// 划词卡片翻译卡片上的原文（输入卡片 ⌘↵、改了原文重翻、换用、下载语言包后重试，M26 规则 16、19、20）。
/// `lang` 为这张卡片选的目标语言代码，null 为自动。`seq` 取自 `trcard-show` 或上一次的返回值；
/// 成功时新序号恒为 `seq + 1`，结果经 `trcard-done` 送达（可能先于本命令返回）。
#[tauri::command]
pub async fn trcard_translate(
    app: AppHandle,
    seq: u64,
    text: String,
    lang: Option<String>,
    engine: Option<String>,
) -> Result<crate::trcard::Again, AppError> {
    let lang = match lang.as_deref() {
        None => None,
        Some(c) => {
            Some(Lang::from_code(c).ok_or_else(|| AppError::Invalid("目标语言不对".into()))?)
        }
    };
    crate::trcard::translate(&app, seq, text, lang, parse_engine(engine.as_deref())?)
}

/// 划词卡片“复制译文”：普通写入剪切板，进历史。
#[tauri::command(async)]
pub fn trcard_copy(text: String) -> Result<(), AppError> {
    crate::trcard::copy(&text)
}

/// 关闭划词卡片（Esc、复制后、提示到时）：中止进行中的请求，键盘交还原处。不是当前的 `seq` 时忽略。
#[tauri::command]
pub fn trcard_close(app: AppHandle, seq: u64) {
    crate::trcard::close(&app, seq);
}

/// 划词卡片窗口失去键盘（点了别处）时由前端调用：关闭，不交还键盘。不是当前的 `seq` 时忽略。
#[tauri::command]
pub fn trcard_blur(app: AppHandle, seq: u64) {
    crate::trcard::blur(&app, seq);
}

/// 划词卡片里下载语言包取消或失败后重新拿键盘。不是当前的 `seq` 时忽略。
#[tauri::command]
pub fn trcard_focus(app: AppHandle, seq: u64) {
    crate::trcard::focus(&app, seq);
}

/// 前端渲染后按内容改卡片高度（逻辑像素），`wide` 为展开对照（加宽成两栏），`moved` 为用户拖动过卡片
/// （之后左上角留在原处）。不是当前的 `seq` 时忽略。
#[tauri::command]
pub fn trcard_resize(app: AppHandle, seq: u64, h: f64, wide: bool, moved: bool) {
    crate::trcard::resize(&app, seq, h, wide, moved);
}
