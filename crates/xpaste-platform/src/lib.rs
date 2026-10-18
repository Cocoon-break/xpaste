//! xPaste 平台层：macOS / Windows 系统接口。不依赖 Tauri。

pub mod error;
pub use error::PlatformError;

/// 模拟粘贴前的检查结果：不是 `None` 时只复制，提示用户手动粘贴。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteBlock {
    None,
    /// macOS 未授予辅助功能权限
    NeedAccessibility,
    /// Windows 目标窗口权限高于本进程（UIPI 会丢弃模拟按键）
    Elevated,
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(test)]
mod tests {
    #[test]
    fn os_is_a_supported_platform() {
        assert!(matches!(super::OS, "macos" | "windows"));
    }
}
