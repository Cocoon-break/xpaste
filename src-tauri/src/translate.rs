//! 翻译分发：OpenAI 兼容接口流式请求与系统翻译（M26 规则 7、8）。
//! 日志只记 id、字数、耗时、状态码与错误类别，不写原文、译文、API Key、Base URL。

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::Serialize;
use xpaste_core::Settings;
pub use xpaste_core::translate::Lang;
use xpaste_core::translate::{
    MAX_CHARS, SseLine, chat_body, completion_text, endpoint, http_error, is_loopback,
    resolve_target, sse_line,
};

use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    System,
    OpenAi,
}

impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Engine::System => "system",
            Engine::OpenAi => "openai",
        }
    }

    /// 翻译方式标签（`system` / `openai:{模型}`）对应的方式，设计 §3.4。
    pub fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "system" => Some(Engine::System),
            t if t.starts_with("openai:") => Some(Engine::OpenAi),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Cfg {
    pub engine: Engine,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub lang: Lang,
}

/// 读设置；引擎还没选（`""`）时本机支持系统翻译就用系统翻译，否则用 OpenAI 兼容接口（M26 规则 6）。
pub fn cfg(s: &Settings) -> Cfg {
    // 没选过，或选了系统翻译但本机用不了（换到 macOS 26 以下）：按本机能力选，与设置页显示一致（M26 规则 6）
    let engine = match s.translate_engine.as_str() {
        "openai" => Engine::OpenAi,
        _ if xpaste_platform::translate_supported() => Engine::System,
        _ => Engine::OpenAi,
    };
    Cfg {
        engine,
        base_url: s.translate_base_url.clone(),
        api_key: s.translate_api_key.clone(),
        model: s.translate_model.clone(),
        lang: Lang::from_code(&s.translate_lang).unwrap_or(Lang::ZhHans),
    }
}

/// 同 `cfg`，`engine` 给了就强制用它，其余字段照设置（设计 §3.4 单次指定）。
pub fn cfg_for(s: &Settings, engine: Option<Engine>) -> Cfg {
    let c = cfg(s);
    match engine {
        Some(e) => Cfg { engine: e, ..c },
        None => c,
    }
}

impl Cfg {
    /// 存库与增量里标注翻译方式的标签（设计 §3.4）。
    pub fn tag(&self) -> String {
        match self.engine {
            Engine::System => "system".into(),
            Engine::OpenAi => format!("openai:{}", self.model.trim()),
        }
    }
}

/// `translate-delta` 的载荷（M26 规则 3、5）。
#[derive(Debug, Clone, Serialize)]
pub struct Delta {
    pub id: Option<i64>,
    pub lang: String,
    /// 翻译方式标签，见 `Cfg::tag`
    pub engine: String,
    pub text: String,
}

/// 增量合并：距上次吐出满 50 ms 才吐一次，结束时 `flush` 发完剩下的（M26 规则 3）。
/// ponytail: 只在新增量到来时检查时间，流停顿期间攒着的部分要等下一段或 flush；本机慢模型一段间隔也远超 50 ms，够用。
#[derive(Debug, Default)]
pub struct Coalescer {
    buf: String,
    last: Option<Instant>,
}

impl Coalescer {
    pub fn push(&mut self, s: &str, now: Instant) -> Option<String> {
        self.buf.push_str(s);
        let last = *self.last.get_or_insert(now);
        if now.duration_since(last) < Duration::from_millis(50) {
            return None;
        }
        self.last = Some(now);
        Some(std::mem::take(&mut self.buf))
    }

    pub fn flush(&mut self) -> Option<String> {
        (!self.buf.is_empty()).then(|| std::mem::take(&mut self.buf))
    }
}

/// 失败原因：转成 `AppError` 给前端，`tag` 写日志。
#[derive(Debug)]
pub enum Fail {
    Config,
    TooLong,
    Http(u16),
    Timeout,
    Connect,
    Bad,
    /// 系统翻译缺语言包，带拼好的提示
    Pack(String),
    Os,
    Unsupported,
    System,
    /// `on_delta` 返回 false（截图已结束）
    Aborted,
    Internal(AppError),
}

impl Fail {
    pub fn tag(&self) -> String {
        match self {
            Fail::Http(s) => format!("http={s}"),
            Fail::Config => "config".into(),
            Fail::TooLong => "toolong".into(),
            Fail::Timeout => "timeout".into(),
            Fail::Connect => "connect".into(),
            Fail::Bad => "bad".into(),
            Fail::Pack(_) => "pack".into(),
            Fail::Os => "os".into(),
            Fail::Unsupported => "unsupported".into(),
            Fail::System => "system".into(),
            Fail::Aborted => "aborted".into(),
            Fail::Internal(_) => "internal".into(),
        }
    }
}

impl From<Fail> for AppError {
    fn from(f: Fail) -> Self {
        let t = |s: &str| AppError::Translate(s.into());
        match f {
            Fail::Config => AppError::TranslateConfig,
            Fail::TooLong => t("内容太长，最多翻译 10000 字"),
            Fail::Http(s) => AppError::Translate(http_error(s)),
            Fail::Timeout => t("翻译超时"),
            Fail::Connect => t("连不上翻译接口，请检查网络或接口地址"),
            Fail::Bad => t("翻译服务返回的内容无法识别"),
            Fail::Pack(msg) => AppError::TranslatePack(msg),
            Fail::Os => AppError::TranslateOs,
            Fail::Unsupported => t("系统翻译不支持这种语言"),
            Fail::System => t("系统翻译失败"),
            Fail::Aborted => AppError::Invalid("翻译已取消".into()),
            Fail::Internal(e) => e,
        }
    }
}

/// 翻译 `text`：返回实际翻成的语言与去掉首尾空白的完整译文。
/// 增量经 `on_delta(实际语言, 新增部分)` 交出，它返回 false 时中止（截图已结束时用）。
pub async fn run(
    cfg: &Cfg,
    text: &str,
    on_delta: impl FnMut(Lang, &str) -> bool + Send,
) -> Result<(Lang, String), AppError> {
    Ok(exec(cfg, text, None, on_delta).await?)
}

/// OpenAI 兼容接口的超时：连接 10 秒；读空闲 30 秒，首段与段间都算，没有总时长上限（M26 规则 10）。
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub connect: Duration,
    pub read: Duration,
}

const TIMEOUTS: Timeouts = Timeouts {
    connect: Duration::from_secs(10),
    read: Duration::from_secs(30),
};

/// 未配置：用 OpenAI 兼容接口，且接口地址或模型 ID 为空（M26 规则 6）。
pub fn unconfigured(cfg: &Cfg) -> bool {
    cfg.engine == Engine::OpenAi && (cfg.model.trim().is_empty() || cfg.base_url.trim().is_empty())
}

/// `run` 的实现；`timeouts` 为 None 时用正式客户端（见 `client`），测试传短超时另建客户端。
pub async fn exec(
    cfg: &Cfg,
    text: &str,
    timeouts: Option<Timeouts>,
    mut on_delta: impl FnMut(Lang, &str) -> bool + Send,
) -> Result<(Lang, String), Fail> {
    if unconfigured(cfg) {
        return Err(Fail::Config);
    }
    if text.chars().count() > MAX_CHARS {
        return Err(Fail::TooLong);
    }
    let target = resolve_target(text, cfg.lang);
    let out = match cfg.engine {
        Engine::System => {
            let out = system(text, target).await?;
            if !on_delta(target, &out) {
                return Err(Fail::Aborted);
            }
            out
        }
        Engine::OpenAi => openai(cfg, text, target, timeouts, &mut on_delta).await?,
    };
    let out = out.trim();
    if out.is_empty() {
        return Err(Fail::Bad);
    }
    Ok((target, out.to_string()))
}

/// 系统翻译（整段返回）。macOS Translation 靠主队列回调：不能在主线程上等（会卡死到超时），
/// 也不占 tokio 工作线程，所以放到阻塞线程池（M26 规则 7）。
async fn system(text: &str, target: Lang) -> Result<String, Fail> {
    use xpaste_platform::SysTranslateError as E;
    let src = text.to_string();
    let r = tauri::async_runtime::spawn_blocking(move || {
        xpaste_platform::translate_text(&src, target.code())
    })
    .await
    .map_err(|e| Fail::Internal(e.into()))?;
    r.map_err(|e| match e {
        E::OsTooOld => Fail::Os,
        E::NotInstalled { source } => {
            let from = Lang::from_code(&source).map_or(source, |l| l.label().to_string());
            Fail::Pack(format!("需要先下载{from}→{}的翻译语言包", target.label()))
        }
        E::Unsupported => Fail::Unsupported,
        E::Timeout => Fail::Timeout,
        E::Failed => Fail::System,
    })
}

fn build_client(loopback: bool, t: Timeouts) -> Result<reqwest::Client, Fail> {
    let b = reqwest::Client::builder()
        .connect_timeout(t.connect)
        .read_timeout(t.read);
    // 本机地址始终直连；其余跟随系统代理（`system-proxy` feature，M26 规则 11）
    let b = if loopback { b.no_proxy() } else { b };
    b.build().map_err(|e| {
        log::warn!("翻译 HTTP 客户端创建失败：{e}");
        Fail::Connect
    })
}

/// 正式客户端。reqwest 只在 build 时读一次系统代理，所以非本机地址每次现建，
/// 用户中途开关代理也能跟上（M26 规则 11）；翻译由用户触发，建客户端的开销可以忽略。本机地址不走代理，缓存一个。
fn client(loopback: bool) -> Result<reqwest::Client, Fail> {
    static LOOPBACK: OnceLock<reqwest::Client> = OnceLock::new();
    if !loopback {
        return build_client(false, TIMEOUTS);
    }
    if let Some(c) = LOOPBACK.get() {
        return Ok(c.clone());
    }
    let c = build_client(true, TIMEOUTS)?;
    Ok(LOOPBACK.get_or_init(|| c).clone())
}

fn net(e: reqwest::Error) -> Fail {
    if e.is_timeout() {
        Fail::Timeout
    } else {
        // 连接失败、DNS 失败，以及读到一半连接断开
        Fail::Connect
    }
}

async fn openai(
    cfg: &Cfg,
    text: &str,
    target: Lang,
    timeouts: Option<Timeouts>,
    on_delta: &mut (impl FnMut(Lang, &str) -> bool + Send),
) -> Result<String, Fail> {
    let loopback = is_loopback(&cfg.base_url);
    let client = match timeouts {
        Some(t) => build_client(loopback, t)?,
        None => client(loopback)?,
    };
    let mut req =
        client
            .post(endpoint(&cfg.base_url))
            .json(&chat_body(cfg.model.trim(), target, text));
    let key = cfg.api_key.trim();
    if !key.is_empty() {
        req = req.bearer_auth(key);
    }
    let mut resp = req.send().await.map_err(net)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Fail::Http(status.as_u16()));
    }
    let sse = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/event-stream"));
    if !sse {
        // 忽略 stream 直接回整段 JSON 的兼容服务：当作一段一次性给出
        let body = resp.bytes().await.map_err(net)?;
        let out = completion_text(&String::from_utf8_lossy(&body)).ok_or(Fail::Bad)?;
        if !on_delta(target, &out) {
            return Err(Fail::Aborted);
        }
        return Ok(out);
    }
    let mut out = String::new();
    // 按字节攒到换行再转 UTF-8，拆在两块里的多字节字符不会乱码
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let chunk = resp.chunk().await.map_err(net)?;
        let eof = chunk.is_none();
        match chunk {
            Some(c) => buf.extend_from_slice(&c),
            None if buf.is_empty() => break,
            None => buf.push(b'\n'), // 最后一行没有换行
        }
        while let Some(i) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=i).collect();
            match sse_line(String::from_utf8_lossy(&line).trim_end()) {
                SseLine::Delta(d) => {
                    out.push_str(&d);
                    if !on_delta(target, &d) {
                        return Err(Fail::Aborted);
                    }
                }
                SseLine::Done => return Ok(out),
                SseLine::Bad => return Err(Fail::Bad),
                SseLine::Skip => {}
            }
        }
        if eof {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::{Duration, Instant};

    pub(crate) const SSE_HEAD: &str =
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";

    pub(crate) fn delta(s: &str) -> String {
        format!(
            "data: {}\n\n",
            serde_json::json!({"choices": [{"delta": {"content": s}}]})
        )
    }

    /// 假服务：接一个连接，读完请求，按 `parts`（写之前先睡的毫秒数、字节）分几次写回；
    /// `hold` 为真时写完不关连接（睡 2 秒）。返回 Base URL 与收到的请求体。
    pub(crate) fn serve(
        parts: Vec<(u64, Vec<u8>)>,
        hold: bool,
    ) -> (String, thread::JoinHandle<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/v1", l.local_addr().unwrap().port());
        let h = thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            let body_at = loop {
                let n = s.read(&mut buf).unwrap();
                req.extend_from_slice(&buf[..n]);
                if let Some(i) = req.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i + 4;
                }
            };
            let head = String::from_utf8_lossy(&req[..body_at]).to_ascii_lowercase();
            let len: usize = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .map_or(0, |v| v.trim().parse().unwrap());
            while req.len() < body_at + len {
                let n = s.read(&mut buf).unwrap();
                req.extend_from_slice(&buf[..n]);
            }
            for (ms, bytes) in parts {
                thread::sleep(Duration::from_millis(ms));
                s.write_all(&bytes).unwrap();
                s.flush().unwrap();
            }
            if hold {
                thread::sleep(Duration::from_secs(2));
            }
            String::from_utf8_lossy(&req[body_at..]).into_owned()
        });
        (url, h)
    }

    pub(crate) fn openai(base_url: &str) -> Cfg {
        Cfg {
            engine: Engine::OpenAi,
            base_url: base_url.into(),
            api_key: "k".into(),
            model: "m".into(),
            lang: Lang::ZhHans,
        }
    }

    pub(crate) const FAST: Timeouts = Timeouts {
        connect: Duration::from_secs(2),
        read: Duration::from_millis(300),
    };

    /// 跑一次翻译，返回结果与 on_delta 收到的各段。
    fn go(cfg: &Cfg, text: &str) -> (Result<(Lang, String), AppError>, Vec<String>) {
        let mut got = Vec::new();
        let r = tauri::async_runtime::block_on(exec(cfg, text, Some(FAST), |_, d| {
            got.push(d.to_string());
            true
        }));
        (r.map_err(AppError::from), got)
    }

    fn err_msg(r: Result<(Lang, String), AppError>) -> String {
        r.unwrap_err().to_string()
    }

    #[test]
    fn streams_deltas_in_order() {
        let body = format!(
            "{SSE_HEAD}: ping\n\n{}{}{}data: [DONE]\n\n",
            delta("构建"),
            delta("失败"),
            delta("。")
        );
        let (url, h) = serve(vec![(0, body.into_bytes())], false);
        let (r, got) = go(&openai(&url), "The build failed");
        assert_eq!(got.concat(), "构建失败。");
        assert_eq!(r.unwrap(), (Lang::ZhHans, "构建失败。".to_string()));
        let req: serde_json::Value = serde_json::from_str(&h.join().unwrap()).unwrap();
        assert_eq!(req["stream"], true);
        assert_eq!(req["model"], "m");
    }

    #[test]
    fn stream_survives_split_lines() {
        let body = format!(
            "{SSE_HEAD}{}{}data: [DONE]\n\n",
            delta("构建"),
            delta("失败")
        );
        let bytes = body.into_bytes();
        let cut = bytes.windows(3).position(|w| w == "败".as_bytes()).unwrap() + 1;
        let (url, _h) = serve(
            vec![(0, bytes[..cut].to_vec()), (100, bytes[cut..].to_vec())],
            false,
        );
        let (r, _) = go(&openai(&url), "The build failed");
        assert_eq!(r.unwrap().1, "构建失败");
    }

    #[test]
    fn non_stream_response_is_accepted() {
        let json = r#"{"choices":[{"message":{"content":"你好"}}]}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
            json.len()
        );
        let (url, _h) = serve(vec![(0, resp.into_bytes())], false);
        let (r, got) = go(&openai(&url), "hello");
        assert_eq!(r.unwrap().1, "你好");
        assert_eq!(got, vec!["你好"]);
    }

    #[test]
    fn http_status_maps_to_message() {
        for status in [401u16, 404, 429, 503] {
            let resp =
                format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            let (url, _h) = serve(vec![(0, resp.into_bytes())], false);
            let (r, _) = go(&openai(&url), "hello");
            let e = r.unwrap_err();
            assert_eq!(e.kind(), "translate");
            assert_eq!(e.to_string(), xpaste_core::translate::http_error(status));
        }
    }

    #[test]
    fn empty_result_is_error() {
        for body in [
            "data: [DONE]\n\n".to_string(),
            "data: {\"error\":{\"message\":\"x\"}}\n\n".to_string(),
        ] {
            let (url, _h) = serve(vec![(0, format!("{SSE_HEAD}{body}").into_bytes())], false);
            let (r, _) = go(&openai(&url), "hello");
            let e = r.unwrap_err();
            assert_eq!(e.kind(), "translate");
            assert_eq!(e.to_string(), "翻译服务返回的内容无法识别");
        }
    }

    #[test]
    fn idle_timeout_and_unreachable() {
        let first = format!("{SSE_HEAD}{}", delta("构建"));
        let (url, _h) = serve(vec![(0, first.into_bytes())], true);
        let t0 = Instant::now();
        let (r, got) = go(&openai(&url), "hello");
        assert_eq!(err_msg(r), "翻译超时");
        assert_eq!(got, vec!["构建"]);
        assert!(t0.elapsed() < Duration::from_millis(1500));

        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let (r, _) = go(&openai(&format!("http://127.0.0.1:{port}/v1")), "hello");
        assert_eq!(err_msg(r), "连不上翻译接口，请检查网络或接口地址");
    }

    #[test]
    fn over_limit_and_unconfigured() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let url = format!("http://127.0.0.1:{}/v1", l.local_addr().unwrap().port());
        let (r, _) = go(&openai(&url), &"a".repeat(MAX_CHARS + 1));
        assert_eq!(err_msg(r), "内容太长，最多翻译 10000 字");
        assert!(l.accept().is_err(), "超长不应发请求");

        let mut cfg = openai(&url);
        cfg.model = " ".into();
        let (r, _) = go(&cfg, "hello");
        assert_eq!(r.unwrap_err().kind(), "translateConfig");
        assert!(l.accept().is_err(), "未配置不应发请求");
    }

    #[test]
    fn coalescer_batches_by_50ms() {
        let t = Instant::now();
        let ms = Duration::from_millis;
        let mut c = Coalescer::default();
        assert_eq!(c.push("a", t), None);
        assert_eq!(c.push("b", t + ms(10)), None);
        assert_eq!(c.push("c", t + ms(20)), None);
        assert_eq!(c.push("d", t + ms(60)).as_deref(), Some("abcd"));
        assert_eq!(c.push("e", t + ms(70)), None);
        assert_eq!(c.flush().as_deref(), Some("e"));
        assert_eq!(c.flush(), None);
    }
}
