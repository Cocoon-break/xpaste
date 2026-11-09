//! 检查更新（规格说明 M28）：启动 10 秒后、之后每 24 小时问一次 GitHub 最新正式版。
//! 有新版本时托盘加一项“下载新版本…”，同一版本只发一次系统通知；不自动下载安装。

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::state::AppState;

/// 网页地址而不是 api.github.com：API 不登录时每个出口 IP 每小时只有 60 次，公司网络、代理后面的用户会一直被限流
const LATEST: &str = "https://github.com/Cocoon-break/xpaste/releases/latest";
const FIRST_DELAY: Duration = Duration::from_secs(10);
const EVERY: Duration = Duration::from_secs(24 * 3600);

/// 本次运行发现的新版本号
static FOUND: Mutex<Option<String>> = Mutex::new(None);

/// `tag`（如 `v0.8.0`）比当前版本新时返回去掉 `v` 的版本号；格式不是 `x.y.z` 的不算新版本。
pub fn newer(current: &str, tag: &str) -> Option<String> {
    fn parse(v: &str) -> Option<[u64; 3]> {
        let mut it = v.split('.').map(|p| p.parse().ok());
        let r = [it.next()??, it.next()??, it.next()??];
        it.next().is_none().then_some(r)
    }
    let v = tag.strip_prefix('v').unwrap_or(tag);
    (parse(v)? > parse(current)?).then(|| v.to_string())
}

/// `releases/latest` 跳转到的地址里取标签名；还没有正式版时跳到 Release 列表页，取不到。
pub fn tag_from_location(loc: &str) -> Option<&str> {
    let (_, tag) = loc.split_once("/releases/tag/")?;
    (!tag.is_empty()).then_some(tag)
}

pub fn release_url(v: &str) -> String {
    format!("https://github.com/Cocoon-break/xpaste/releases/tag/v{v}")
}

pub fn found() -> Option<String> {
    FOUND.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// 浏览器打开新版本的 Release 页（托盘“下载新版本…”）。
pub fn open(app: &AppHandle) {
    if let Some(v) = found()
        && let Err(e) = app.opener().open_url(release_url(&v), None::<&str>)
    {
        log::warn!("打开 Release 页失败：{e}");
    }
}

pub fn start(app: AppHandle) {
    crate::housekeeping::spawn("xpaste-update", move || {
        std::thread::sleep(FIRST_DELAY);
        loop {
            check(&app);
            std::thread::sleep(EVERY);
        }
    });
}

/// 设置里打开“自动检查更新”后立刻查一次。
pub fn check_now(app: &AppHandle) {
    let app = app.clone();
    crate::housekeeping::spawn("xpaste-update-now", move || check(&app));
}

fn check(app: &AppHandle) {
    if !app.state::<AppState>().settings().update_check {
        return;
    }
    let current = app.package_info().version.to_string();
    let tag = match tauri::async_runtime::block_on(latest_tag(&current)) {
        Ok(t) => t,
        // 断网、限流都不打扰用户（M28 规则 3）
        Err(e) => {
            log::info!("检查更新失败：{e}");
            return;
        }
    };
    let Some(v) = newer(&current, &tag) else {
        log::info!("检查更新：{current} 已是最新");
        return;
    };
    {
        let mut f = FOUND.lock().unwrap_or_else(PoisonError::into_inner);
        if f.as_deref() == Some(v.as_str()) {
            return;
        }
        *f = Some(v.clone());
    }
    log::info!("检查更新：发现新版本 {v}");
    crate::tray::show_update(&v);
    // 同一版本只通知一次，重启也不再通知（M28 规则 5）
    let state = app.state::<AppState>();
    let notified = state.store().kv_get("update_notified");
    if !matches!(notified, Ok(Some(n)) if n == v) {
        let place = if cfg!(target_os = "macos") {
            "菜单栏"
        } else {
            "托盘"
        };
        crate::notify(app, &format!("新版本 {v} 已发布，可在{place}菜单里下载"));
        if let Err(e) = state.store().kv_set("update_notified", &v) {
            log::warn!("记录已通知的版本失败：{e}");
        }
    }
}

/// 只发 User-Agent 里的版本号，不带剪切板内容、设置或任何标识（M28 规则 2）；跟随系统代理。
/// 不跟随跳转，从 `Location` 读出最新正式版的标签名。
async fn latest_tag(current: &str) -> Result<String, String> {
    let r = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent(format!("xPaste/{current}"))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?
        .head(LATEST)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_redirection() {
        return Err(format!("HTTP {}", r.status()));
    }
    r.headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .and_then(tag_from_location)
        .map(str::to_string)
        .ok_or_else(|| "跳转地址里没有版本号".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_compares_numerically() {
        assert_eq!(newer("0.7.0", "v0.8.0").as_deref(), Some("0.8.0"));
        assert_eq!(newer("0.7.0", "0.7.1").as_deref(), Some("0.7.1"));
        assert_eq!(newer("0.9.0", "v0.10.0").as_deref(), Some("0.10.0"));
        assert_eq!(newer("0.7.0", "v1.0.0").as_deref(), Some("1.0.0"));
        assert_eq!(newer("0.7.0", "v0.7.0"), None);
        assert_eq!(newer("0.10.0", "v0.9.9"), None);
        assert_eq!(newer("0.7.0", "v0.8.0-beta.1"), None);
        assert_eq!(newer("0.7.0", "nightly"), None);
        assert_eq!(newer("0.7.0", "v0.8"), None);
    }

    #[test]
    fn tag_from_redirect() {
        assert_eq!(
            tag_from_location("https://github.com/Cocoon-break/xpaste/releases/tag/v0.7.1"),
            Some("v0.7.1")
        );
        // 还没有任何正式版时跳到 Release 列表页
        assert_eq!(
            tag_from_location("https://github.com/Cocoon-break/xpaste/releases"),
            None
        );
        assert_eq!(
            tag_from_location("https://github.com/Cocoon-break/xpaste/releases/tag/"),
            None
        );
    }

    #[test]
    fn release_url_points_at_tag() {
        assert_eq!(
            release_url("0.8.0"),
            "https://github.com/Cocoon-break/xpaste/releases/tag/v0.8.0"
        );
    }
}
