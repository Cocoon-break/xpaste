//! 检查更新（规格说明 M28）：启动 10 秒后、之后每 24 小时问一次 GitHub 最新正式版。
//! 有新版本时托盘加一项“下载新版本…”，同一版本只发一次系统通知；不自动下载安装。

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::state::AppState;

const LATEST_API: &str = "https://api.github.com/repos/Cocoon-break/xpaste/releases/latest";
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
async fn latest_tag(current: &str) -> Result<String, reqwest::Error> {
    #[derive(serde::Deserialize)]
    struct Release {
        tag_name: String,
    }
    let r: Release = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent(format!("xPaste/{current}"))
        .build()?
        .get(LATEST_API)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(r.tag_name)
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
    fn release_url_points_at_tag() {
        assert_eq!(
            release_url("0.8.0"),
            "https://github.com/Cocoon-break/xpaste/releases/tag/v0.8.0"
        );
    }
}
