//! 对接授权服务：联网激活、每天静默续期、解绑设备。只有带 Pro 的构建、存着授权码的用户才会走到这里。
//! 发出去的只有授权码、设备哈希（不可逆）、电脑名和应用版本，不带剪切板内容。
//! 日志只记状态码与错误类别，不写授权码、邮箱、凭证、设备名。

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};
use xpaste_pro::{Line, Rejection};

use crate::error::AppError;
use crate::state::AppState;

/// 授权服务里的应用标识
const APP: &str = "xpaste";
const FIRST_DELAY: Duration = Duration::from_secs(15);
const EVERY: Duration = Duration::from_secs(24 * 3600);
/// 该续却没续上（断网、限流、服务暂时不可用）时隔这么久再试：离线到凭证过期的用户联网后不用等一整天
const RETRY: Duration = Duration::from_secs(3600);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TIMEOUT: Duration = Duration::from_secs(20);

/// 站点地址（不带末尾斜杠）：接口在 `<站点>/v1/…`，购买页是 `<站点>/buy.html`。
/// 编译期环境变量 `XPASTE_SITE` 给出；开发构建（debug）里运行时的同名环境变量优先。都没有为 None：
/// 不显示购买入口，激活报“授权服务还没开通”
pub fn site() -> Option<String> {
    let runtime = if cfg!(debug_assertions) {
        std::env::var("XPASTE_SITE").ok()
    } else {
        None
    };
    pick_site(runtime.as_deref(), option_env!("XPASTE_SITE"))
}

fn pick_site(runtime: Option<&str>, built: Option<&str>) -> Option<String> {
    [runtime, built]
        .into_iter()
        .flatten()
        .map(|s| s.trim().trim_end_matches('/'))
        .find(|s| s.starts_with("https://") || s.starts_with("http://"))
        .map(str::to_string)
}

pub fn buy_url() -> Option<String> {
    site().map(|s| format!("{s}/buy.html"))
}

pub fn no_site() -> AppError {
    AppError::License("授权服务还没开通".into())
}

/// 授权码当前绑定着的一台设备
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct Device {
    pub id: i64,
    pub name: String,
    /// RFC 3339
    pub activated_at: String,
}

/// 名字不进日志
impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Device({})", self.id)
    }
}

/// 请求没成的原因
#[derive(Debug, PartialEq)]
pub enum Fail {
    /// 授权服务明确拒绝：续期时据此清掉本地凭证
    Rejected(Rejection),
    /// 设备满员，带当前绑定着的设备
    DeviceLimit(Vec<Device>),
    /// 自助解绑次数用完
    UnbindLimit,
    /// 这张授权下没有要解绑的那台设备
    NotFound,
    RateLimited,
    /// 503、其他 5xx 和认不出来的响应：暂时性问题
    Unavailable,
    /// 连不上或超时
    Network,
}

impl Fail {
    /// 日志里用的类别
    fn label(&self) -> &'static str {
        match self {
            Self::Rejected(r) => r.code(),
            Self::DeviceLimit(_) => "device_limit",
            Self::UnbindLimit => "unbind_limit",
            Self::NotFound => "not_found",
            Self::RateLimited => "rate_limited",
            Self::Unavailable => "unavailable",
            Self::Network => "network",
        }
    }
}

impl From<Fail> for AppError {
    fn from(f: Fail) -> Self {
        let text = match f {
            Fail::Rejected(r) => r.message(),
            Fail::DeviceLimit(devices) => return Self::DeviceLimit(devices),
            Fail::UnbindLimit => "解绑次数用完了，请联系我们",
            Fail::NotFound => "这台设备已经不在绑定列表里",
            Fail::RateLimited => "请求太频繁，稍后再试",
            Fail::Unavailable => "授权服务暂时不可用，稍后再试",
            Fail::Network => "连不上授权服务，请检查网络",
        };
        Self::License(text.into())
    }
}

/// 非 200 的响应归类：只看状态码和 `error` 字段，不匹配 `message`。
/// 状态码对但 `error` 认不出来的（比如代理自己回的 403、404 页面）一律当暂时性问题，不当成拒绝
fn classify(status: u16, body: &[u8]) -> Fail {
    let v: Value = serde_json::from_slice(body).unwrap_or_default();
    let error = v["error"].as_str().unwrap_or_default();
    if let (403 | 404, Some(why)) = (status, Rejection::from_code(error)) {
        return Fail::Rejected(why);
    }
    match (status, error) {
        (404, "not_found") => Fail::NotFound,
        (409, "device_limit") => match serde_json::from_value(v["devices"].clone()) {
            Ok(devices) => Fail::DeviceLimit(devices),
            Err(_) => Fail::Unavailable,
        },
        (429, "unbind_limit") => Fail::UnbindLimit,
        (429, _) => Fail::RateLimited,
        _ => Fail::Unavailable,
    }
}

/// 跟随系统代理（本机地址直连，联调用）；reqwest 只在 build 时读一次系统代理，所以每次现建
async fn post(site: &str, path: &str, body: Value) -> Result<Value, Fail> {
    let b = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TIMEOUT)
        .user_agent(concat!("xPaste/", env!("CARGO_PKG_VERSION")));
    let b = if xpaste_core::translate::is_loopback(site) {
        b.no_proxy()
    } else {
        b
    };
    let sent = async {
        let r = b
            .build()?
            .post(format!("{site}{path}"))
            .json(&body)
            .send()
            .await?;
        let status = r.status().as_u16();
        Ok::<_, reqwest::Error>((status, r.bytes().await?))
    };
    let (status, bytes) = sent.await.map_err(|e| {
        // without_url：错误里的地址不进日志
        log::info!("授权服务请求失败：{}", e.without_url());
        Fail::Network
    })?;
    if status != 200 {
        let f = classify(status, &bytes);
        log::info!("授权服务 {path} 返回 {status}（{}）", f.label());
        return Err(f);
    }
    serde_json::from_slice(&bytes).map_err(|_| Fail::Unavailable)
}

/// 激活（`renew` 为 false）或续期（true），成功返回凭证。`code` 是规整后的授权码，`device` 是本机设备哈希
pub async fn activate(site: &str, code: &str, device: &str, renew: bool) -> Result<String, Fail> {
    let name = xpaste_platform::device_name().unwrap_or_else(|| {
        if cfg!(target_os = "macos") {
            "Mac".into()
        } else {
            "Windows 电脑".into()
        }
    });
    let body = json!({
        "app": APP, "code": code, "device": device, "device_name": name,
        "app_version": env!("CARGO_PKG_VERSION"), "renew": renew,
    });
    let v = post(site, "/v1/activate", body).await?;
    v["credential"]
        .as_str()
        .map(str::to_string)
        .ok_or(Fail::Unavailable)
}

/// 要解绑哪台：别的设备用列表里的 id，本机用设备哈希
pub enum Unbind<'a> {
    Id(i64),
    This(&'a str),
}

pub async fn deactivate(site: &str, code: &str, which: Unbind<'_>) -> Result<(), Fail> {
    let body = match which {
        Unbind::Id(id) => json!({"app": APP, "code": code, "device_id": id}),
        Unbind::This(device) => json!({"app": APP, "code": code, "device": device}),
    };
    post(site, "/v1/deactivate", body).await.map(|_| ())
}

/// 授权码当前绑定着的设备。界面目前用不到（满员时的 409 已经带了列表），留给联调检查用
#[cfg_attr(not(test), allow(dead_code))]
pub async fn devices(site: &str, code: &str) -> Result<Vec<Device>, Fail> {
    let v = post(site, "/v1/devices", json!({"app": APP, "code": code})).await?;
    serde_json::from_value(v["devices"].clone()).map_err(|_| Fail::Unavailable)
}

fn lock(line: &Mutex<Line>) -> std::sync::MutexGuard<'_, Line> {
    line.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 一次续期检查的结果
#[derive(Debug, PartialEq)]
pub enum Renewal {
    /// 还不用续，或没有授权码：没发请求
    NotDue,
    /// 收下了新凭证，或被明确拒绝、清掉了凭证：授权状态可能变了
    Changed,
    /// 该续却没续上（断网、限流、暂时不可用）：什么都没动，稍后再试
    Failed,
}

/// 该续期了就续一次。持锁期间不做网络请求。
/// 成功收下新凭证；授权服务明确拒绝就清掉凭证并记下原因；限流、暂时不可用、连不上都不动，下次再试。
/// 没有授权码的用户在第一步就返回，不发任何请求
pub async fn renew_once(site: &str, line: &Mutex<Line>, now: i64) -> Renewal {
    let Some(code) = lock(line).renewal(now) else {
        return Renewal::NotDue;
    };
    let device = match lock(line).device() {
        Ok(d) => d,
        Err(e) => {
            log::warn!("授权续期：{}", e.message());
            return Renewal::Failed;
        }
    };
    match activate(site, &code, &device, true).await {
        Ok(credential) => {
            let mut l = lock(line);
            // 请求在路上时用户换了授权码或停用了：这张凭证不要了
            if l.code().as_deref() != Some(code.as_str()) {
                return Renewal::NotDue;
            }
            match l.accept(&code, &credential, now) {
                Ok(_) => log::info!("授权已续期"),
                Err(e) => log::warn!("授权续期：新凭证没收下：{}", e.message()),
            }
            Renewal::Changed
        }
        Err(Fail::Rejected(why)) => {
            lock(line).reject(&code, why);
            log::info!("授权续期被拒绝（{}），已清除本机凭证", why.code());
            Renewal::Changed
        }
        // 已在 post 里记过日志
        Err(_) => Renewal::Failed,
    }
}

/// 启动 15 秒后（不挡启动）和之后每 24 小时看一次要不要续期；该续没续上时一小时后再试
pub fn start(app: AppHandle) {
    if !crate::line::enabled() {
        return;
    }
    crate::housekeeping::spawn("xpaste-license", move || {
        std::thread::sleep(FIRST_DELAY);
        loop {
            let mut wait = EVERY;
            if let Some(site) = site() {
                let state = app.state::<AppState>();
                let now = crate::clock::now_ms() / 1000;
                match tauri::async_runtime::block_on(renew_once(&site, &state.line, now)) {
                    Renewal::Changed => crate::commands::line::changed(&app),
                    Renewal::Failed => wait = RETRY,
                    Renewal::NotDue => {}
                }
            }
            std::thread::sleep(wait);
        }
    });
}

/// `xpaste://activate?code=…` 带来的授权码，等面板来取（冷启动时面板可能还没订阅事件）。不进日志
static LINK_CODE: Mutex<Option<String>> = Mutex::new(None);

/// 只认 `activate` 这一个动作；授权码只按长度和字符集做基本检查，之后走正常的激活流程
pub fn link_code(url: &tauri::Url) -> Option<String> {
    if url.scheme() != "xpaste" || url.host_str() != Some("activate") {
        return None;
    }
    let (_, code) = url.query_pairs().find(|(k, _)| k == "code")?;
    let ok = (1..=64).contains(&code.len())
        && code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == ' ');
    ok.then(|| code.into_owned())
}

/// 收到 `xpaste://` 链接：唤起面板，让它打开设置的“Pro”标签并激活。不含 Pro 的构建什么都不做
pub fn opened(app: &AppHandle, urls: &[tauri::Url]) {
    if !crate::line::enabled() {
        return;
    }
    let Some(code) = urls.iter().find_map(link_code) else {
        return;
    };
    log::info!("收到激活链接");
    *LINK_CODE.lock().unwrap_or_else(PoisonError::into_inner) = Some(code);
    let app2 = app.clone();
    // 面板的显示要在主线程上做
    if let Err(e) = app.run_on_main_thread(move || {
        crate::panel::show(&app2, None, crate::panel::Trigger::Tray);
        crate::events::emit(&app2, crate::events::PRO_LINK, ());
    }) {
        log::warn!("激活链接没能唤起面板：{e}");
    }
}

/// 面板取走链接带来的授权码（只给一次）
pub fn take_link_code() -> Option<String> {
    LINK_CODE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_prefers_runtime_and_drops_trailing_slash() {
        assert_eq!(pick_site(None, None), None);
        assert_eq!(pick_site(Some(""), Some("  ")), None);
        assert_eq!(
            pick_site(None, Some("https://xpaste.example.com/")).as_deref(),
            Some("https://xpaste.example.com")
        );
        assert_eq!(
            pick_site(Some("http://localhost:18080"), Some("https://x.example")).as_deref(),
            Some("http://localhost:18080")
        );
        // 不是 http(s) 的不认，退回编译期的
        assert_eq!(
            pick_site(Some("ftp://x"), Some("https://x.example")).as_deref(),
            Some("https://x.example")
        );
    }

    #[test]
    fn classify_follows_status_and_error_only() {
        let c = |s, b: &str| classify(s, b.as_bytes());
        for (status, code, r) in [
            (403, "revoked", Rejection::Revoked),
            (403, "expired", Rejection::Expired),
            (403, "device_unbound", Rejection::DeviceUnbound),
            (404, "invalid_code", Rejection::InvalidCode),
        ] {
            let body = format!(r#"{{"error":"{code}","message":"随便写"}}"#);
            assert!(c(status, &body) == Fail::Rejected(r), "{code}");
        }
        assert!(c(404, r#"{"error":"not_found"}"#) == Fail::NotFound);
        assert!(c(429, r#"{"error":"unbind_limit"}"#) == Fail::UnbindLimit);
        assert!(c(429, r#"{"error":"rate_limited"}"#) == Fail::RateLimited);
        assert!(c(429, "") == Fail::RateLimited);
        // 暂时性问题：不能当成拒绝，否则续期会误清凭证
        for (status, body) in [
            (503, r#"{"error":"unavailable"}"#),
            (500, r#"{"error":"internal"}"#),
            (502, "<html>Bad Gateway</html>"),
            (400, r#"{"error":"invalid_request"}"#),
            // 代理或网关自己回的 403、404
            (403, "<html>Forbidden</html>"),
            (403, r#"{"error":"forbidden"}"#),
            (404, "404 page not found"),
            // 拒绝的错误码配上别的状态码
            (500, r#"{"error":"revoked"}"#),
            (409, r#"{"error":"device_limit"}"#),
        ] {
            assert!(c(status, body) == Fail::Unavailable, "{status} {body}");
        }
    }

    #[test]
    fn device_limit_carries_devices() {
        let body = r#"{"error":"device_limit","message":"设备数量已达上限","devices":[{"id":12,"name":"MacBook Pro","activated_at":"2026-10-02T08:15:30Z"}]}"#;
        let Fail::DeviceLimit(d) = classify(409, body.as_bytes()) else {
            panic!("应为 DeviceLimit");
        };
        assert_eq!((d[0].id, d[0].name.as_str()), (12, "MacBook Pro"));
        // 交给前端时是 camelCase
        assert_eq!(
            serde_json::to_value(AppError::from(Fail::DeviceLimit(d))).unwrap(),
            json!({"kind": "deviceLimit", "message": "设备数量已达上限，先解绑一台不用的",
                   "devices": [{"id": 12, "name": "MacBook Pro", "activatedAt": "2026-10-02T08:15:30Z"}]})
        );
    }

    #[test]
    fn fails_map_to_user_messages() {
        let msg = |f: Fail| AppError::from(f).to_string();
        assert_eq!(msg(Fail::Rejected(Rejection::InvalidCode)), "授权码无效");
        assert_eq!(msg(Fail::Rejected(Rejection::Revoked)), "这个授权码已停用");
        assert_eq!(msg(Fail::Rejected(Rejection::Expired)), "订阅已到期");
        assert_eq!(
            msg(Fail::Rejected(Rejection::DeviceUnbound)),
            "这台设备已被解绑"
        );
        assert_eq!(msg(Fail::UnbindLimit), "解绑次数用完了，请联系我们");
        assert_eq!(msg(Fail::RateLimited), "请求太频繁，稍后再试");
        assert_eq!(msg(Fail::Network), "连不上授权服务，请检查网络");
        assert_eq!(msg(Fail::Unavailable), "授权服务暂时不可用，稍后再试");
        assert_eq!(no_site().to_string(), "授权服务还没开通");
    }

    #[test]
    fn link_only_accepts_activate_with_a_plausible_code() {
        let code = |u: &str| link_code(&tauri::Url::parse(u).unwrap());
        assert_eq!(
            code("xpaste://activate?code=XP-Y3PZC-XY28W-3Q5B5-J92PF").as_deref(),
            Some("XP-Y3PZC-XY28W-3Q5B5-J92PF")
        );
        assert_eq!(
            code("xpaste://activate?x=1&code=xp+y3pzc").as_deref(),
            Some("xp y3pzc")
        );
        for bad in [
            "xpaste://activate",
            "xpaste://activate?code=",
            "xpaste://open?code=XP-Y3PZC-XY28W-3Q5B5-J92PF",
            "xpaste://activate/x?code=%3Cscript%3E",
            "https://activate?code=XP-Y3PZC-XY28W-3Q5B5-J92PF",
            &format!("xpaste://activate?code={}", "A".repeat(65)),
        ] {
            assert_eq!(code(bad), None, "{bad}");
        }
    }

    /// 对着真的授权服务走一遍：下单 → 模拟付款 → 激活 → 满员 → 解绑 → 重新激活 → 续期 → 吊销 → 续期被拒。
    /// 默认不跑（CI 不依赖它）。跑法见 CONTRIBUTING.md“对着本机的授权服务联调”
    #[test]
    #[ignore = "要本机联调的授权服务，见 CONTRIBUTING.md"]
    fn e2e_against_local_issuance() {
        if !xpaste_pro::AVAILABLE {
            eprintln!("这个构建不含 Pro，跳过");
            return;
        }
        let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("要设环境变量 {k}"));
        let site = site().expect("要设 XPASTE_SITE");
        let internal = env("XPASTE_E2E_INTERNAL");
        let secret = env("XPASTE_E2E_SITE_SECRET");
        let admin = env("XPASTE_E2E_ADMIN");
        let (user, password) = admin
            .split_once(':')
            .expect("XPASTE_E2E_ADMIN 形如 用户名:密码");
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let nonce = std::process::id();
        let dir = std::env::temp_dir().join(format!("xpaste-license-e2e-{nonce}"));
        let _ = std::fs::remove_dir_all(&dir);
        // 三台假设备：硬件标识不同，各用各的数据目录
        fn hw_a() -> Option<String> {
            Some(format!("e2e-a-{}", std::process::id()))
        }
        fn hw_b() -> Option<String> {
            Some(format!("e2e-b-{}", std::process::id()))
        }
        fn hw_c() -> Option<String> {
            Some(format!("e2e-c-{}", std::process::id()))
        }
        let real_now = crate::clock::now_ms() / 1000;
        let open = |name: &str, hw: fn() -> Option<String>| {
            let d = dir.join(name);
            std::fs::create_dir_all(&d).unwrap();
            Mutex::new(Line::open(&d, real_now, 1, hw))
        };
        let (a, b, c) = (open("a", hw_a), open("b", hw_b), open("c", hw_c));
        let licensed = |l: &Mutex<Line>, now: i64| {
            matches!(lock(l).status(now), xpaste_pro::Pro::Licensed { .. })
        };

        tauri::async_runtime::block_on(async {
            // 下单（mock 渠道）→ 模拟付款 → 查单拿授权码
            let order: Value = http
                .post(format!("{internal}/v1/site/orders"))
                .bearer_auth(&secret)
                .json(
                    &json!({"product": "pro-1x", "email": format!("e2e-{nonce}@example.com"),
                              "provider": "mock", "currency": "CNY"}),
                )
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let order_id = order["order_id"].as_str().expect("下单成功").to_string();
            let paid = http
                .post(format!("{site}/v1/webhooks/mock"))
                .header("X-Mock-Signature", "mock")
                .json(
                    &json!({"id": format!("evt-{order_id}"), "type": "paid", "order_id": order_id}),
                )
                .send()
                .await
                .unwrap();
            assert_eq!(paid.status(), 200);
            let order: Value = http
                .get(format!("{internal}/v1/site/orders/{order_id}"))
                .bearer_auth(&secret)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(order["status"], "paid");
            // 用户手敲的样子：小写、没有连字符
            let typed = order["license_code"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .replace('-', " ");
            let code = xpaste_pro::normalize_code(&typed).expect("授权码规整");
            assert_eq!(Some(code.as_str()), order["license_code"].as_str());
            eprintln!("1 下单、付款、拿到授权码");

            // 乱写的授权码：404 invalid_code
            let dev_a = lock(&a).device().unwrap();
            let bogus = xpaste_pro::normalize_code("XP-00000-00000-00000-00000").unwrap();
            assert!(
                activate(&site, &bogus, &dev_a, false).await
                    == Err(Fail::Rejected(Rejection::InvalidCode))
            );

            // 第一台、第二台激活：凭证用内置（或 XPASTE_LICENSE_PUBKEY 给的）公钥验过才收下
            for l in [&a, &b] {
                let device = lock(l).device().unwrap();
                let cred = activate(&site, &code, &device, false)
                    .await
                    .expect("激活成功");
                assert!(matches!(
                    lock(l).accept(&code, &cred, real_now),
                    Ok(xpaste_pro::Pro::Licensed { major: Some(1), .. })
                ));
            }
            // 重开后离线也是已授权
            let again = Line::open(&dir.join("a"), real_now, 1, hw_a);
            assert!(matches!(
                again.status(real_now),
                xpaste_pro::Pro::Licensed { .. }
            ));
            // 换一台电脑拿这份数据目录：凭证不认
            let moved = Line::open(&dir.join("a"), real_now, 1, hw_b);
            assert!(!matches!(
                moved.status(real_now),
                xpaste_pro::Pro::Licensed { .. }
            ));
            // 同一台重复激活不多占名额
            assert!(activate(&site, &code, &dev_a, false).await.is_ok());
            eprintln!("2 两台设备激活，凭证离线验过");

            // 第三台：满员，带回已绑定的两台
            let dev_c = lock(&c).device().unwrap();
            let Err(Fail::DeviceLimit(bound)) = activate(&site, &code, &dev_c, false).await else {
                panic!("第三台应当满员");
            };
            assert_eq!(bound.len(), 2);
            eprintln!("3 第三台设备满员，列表里有 {} 台", bound.len());

            // 解绑最早的那台（a），第三台重新激活成功
            assert!(
                deactivate(&site, &code, Unbind::Id(bound[0].id))
                    .await
                    .is_ok()
            );
            assert!(deactivate(&site, &code, Unbind::Id(bound[0].id)).await == Err(Fail::NotFound));
            let left = devices(&site, &code).await.unwrap();
            assert!(left.len() == 1 && left[0] == bound[1]);
            let cred = activate(&site, &code, &dev_c, false)
                .await
                .expect("解绑后激活成功");
            assert!(lock(&c).accept(&code, &cred, real_now).is_ok());
            eprintln!("4 解绑一台后第三台激活成功");

            // 一天之内不续；两天后该续了
            assert_eq!(
                renew_once(&site, &b, real_now + 3600).await,
                Renewal::NotDue
            );
            let later = real_now + 2 * 86_400;
            // b 还绑着：续期成功，凭证换新
            assert_eq!(renew_once(&site, &b, later).await, Renewal::Changed);
            assert!(licensed(&b, later));
            // a 已被解绑：续期被拒，凭证清掉，不会被重新绑上
            assert!(licensed(&a, later));
            assert_eq!(renew_once(&site, &a, later).await, Renewal::Changed);
            assert!(!licensed(&a, later) && lock(&a).code().is_none());
            assert_eq!(devices(&site, &code).await.unwrap().len(), 2);
            eprintln!("5 续期：绑着的换了新凭证，被解绑的清了凭证");

            // 在本机停用（解绑本机）
            let dev_b = lock(&b).device().unwrap();
            assert!(deactivate(&site, &code, Unbind::This(&dev_b)).await.is_ok());
            assert_eq!(lock(&b).forget(), Ok(()));
            assert!(!licensed(&b, later));
            assert_eq!(renew_once(&site, &b, later).await, Renewal::NotDue);
            // 授权服务连不上：什么都不动，凭证留着
            assert_eq!(
                renew_once("http://127.0.0.1:9", &c, later).await,
                Renewal::Failed
            );
            assert!(licensed(&c, later));

            // 管理后台吊销 → c 续期被拒、凭证被清
            let login = http
                .post(format!("{internal}/admin/api/login"))
                .json(&json!({"username": user, "password": password}))
                .send()
                .await
                .unwrap();
            assert_eq!(login.status(), 200, "管理后台登录");
            let cookie = login.headers()[reqwest::header::SET_COOKIE]
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_string();
            let found: Value = http
                .get(format!("{internal}/admin/api/licenses?q={code}"))
                .header(reqwest::header::COOKIE, &cookie)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let id = found["items"][0]["id"]
                .as_i64()
                .expect("后台查得到这张授权");
            let revoked = http
                .post(format!("{internal}/admin/api/licenses/{id}/revoke"))
                .header(reqwest::header::COOKIE, &cookie)
                .send()
                .await
                .unwrap();
            assert_eq!(revoked.status(), 200, "吊销");
            assert!(licensed(&c, later));
            assert_eq!(renew_once(&site, &c, later).await, Renewal::Changed);
            assert!(!licensed(&c, later) && lock(&c).renewal(later).is_none());
            // 重开后也是清掉的；试用过期后状态行写原因
            let again = Line::open(&dir.join("c"), real_now, 1, hw_c);
            let after_trial = real_now + 15 * 86_400;
            assert!(matches!(
                again.status(after_trial),
                xpaste_pro::Pro::Expired { reason: Some(r) } if r == "这个授权码已停用"
            ));
            assert!(
                activate(&site, &code, &dev_c, false).await
                    == Err(Fail::Rejected(Rejection::Revoked))
            );
            eprintln!("6 吊销后续期被拒，凭证已清");
        });
        let _ = std::fs::remove_dir_all(&dir);
    }
}
