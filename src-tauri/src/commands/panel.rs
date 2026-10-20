use std::sync::atomic::Ordering;

use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::error::AppError;
use crate::state::AppState;

/// `animate`：前端没有开减少动态效果；macOS 系统材质下据此决定是否原生淡出整个窗口。
#[tauri::command(async)]
pub fn hide_panel(app: AppHandle, animate: bool) -> Result<(), AppError> {
    crate::panel::hide_animated(&app, animate);
    Ok(())
}

/// 前端在对话框打开 / 全部关闭时同步；只写原子变量，同步命令即可。
#[tauri::command]
pub fn set_dialog_open(state: State<'_, AppState>, open: bool) {
    state.dialog_open.store(open, Ordering::SeqCst);
}

/// 系统设置中的辅助功能页（非 macOS 无操作）。AppKit 调用切到主线程。
#[tauri::command(async)]
pub fn open_accessibility_settings(app: AppHandle) -> Result<(), AppError> {
    app.run_on_main_thread(xpaste_platform::open_accessibility_settings)?;
    Ok(())
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
