use std::sync::atomic::Ordering;

use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::error::AppError;
use crate::state::AppState;

/// `animate`：前端没有开减少动态效果；macOS 系统材质下据此决定是否原生淡出整个窗口。
#[tauri::command(async)]
pub fn hide_panel(app: AppHandle, animate: bool) -> Result<(), AppError> {
    crate::panel::hide_animated(&app, animate);
    Ok(())
}

/// 网页已渲染好启动数据：放行面板显示并补上积压的唤起（见 `panel::frontend_ready`）。
#[tauri::command]
pub fn frontend_ready(app: AppHandle) {
    crate::panel::frontend_ready(&app);
}

/// 前端在对话框打开 / 全部关闭时同步；只写原子变量，同步命令即可。
///
/// 只在 true→false 的转变时补一次 `hide_if_unfocused`：对话框在用户切走之后才关（Esc 收起计时、异步保存），
/// 当时的失焦已被忽略，这里补隐藏，免得面板浮在别的应用上。原本就是 false 时（前端启动时的 `syncDialogOpen()`）
/// 不隐藏，否则首次运行时可见但未取得焦点的引导面板会被藏起来。
#[tauri::command]
pub fn set_dialog_open(app: AppHandle, open: bool) {
    let state = app.state::<AppState>();
    if open {
        state.dialog_open.store(true, Ordering::SeqCst);
    } else if state.dialog_open.swap(false, Ordering::SeqCst) {
        crate::panel::hide_if_unfocused(&app);
    }
}

/// 请求系统辅助功能授权对话框并打开系统设置的辅助功能页（已授权或非 macOS 无操作）。AppKit 调用切到主线程。
#[tauri::command(async)]
pub fn request_accessibility(app: AppHandle) -> Result<(), AppError> {
    app.run_on_main_thread(xpaste_platform::request_accessibility)?;
    Ok(())
}

/// 是否已获得粘贴所需的辅助功能权限（Windows 恒为 true）。
#[tauri::command(async)]
pub fn accessibility_granted() -> Result<bool, AppError> {
    Ok(xpaste_platform::can_paste(None) == xpaste_platform::PasteBlock::None)
}

/// 请求屏幕录制授权并打开系统设置的对应页（已授权或非 macOS 无操作）。理由与线程要求同 `request_accessibility`。
#[tauri::command(async)]
pub fn request_screen_capture(app: AppHandle) -> Result<(), AppError> {
    app.run_on_main_thread(xpaste_platform::request_screen_capture)?;
    Ok(())
}

/// 是否已有截图所需的屏幕录制权限（Windows 恒为 true）。
#[tauri::command(async)]
pub fn screen_capture_granted() -> Result<bool, AppError> {
    Ok(xpaste_platform::screen_capture_granted())
}

/// 只认小写 http/https 前缀（与 detect 的链接判定一致）。
pub fn url_is_openable(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// 用系统默认浏览器打开链接。
#[tauri::command(async)]
pub fn open_url(app: AppHandle, url: String) -> Result<(), AppError> {
    if !url_is_openable(&url) {
        return Err(AppError::Invalid("只能打开 http/https 链接".into()));
    }
    app.opener().open_url(url, None::<&str>)?;
    Ok(())
}

/// 在访达 / 资源管理器中显示文件。
#[tauri::command(async)]
pub fn reveal_file(app: AppHandle, path: String) -> Result<(), AppError> {
    if !std::path::Path::new(&path).exists() {
        return Err(AppError::FileMissing);
    }
    app.opener().reveal_item_in_dir(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_urls_are_openable() {
        assert!(url_is_openable("https://example.com/a?b=c"));
        assert!(url_is_openable("http://localhost:1420"));
        assert!(!url_is_openable("javascript:alert(1)"));
        assert!(!url_is_openable("file:///etc/passwd"));
        assert!(!url_is_openable("HTTPS://x")); // 规格里链接判定区分大小写前缀，与 detect 一致
    }
}
