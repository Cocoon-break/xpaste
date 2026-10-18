//! xPaste 平台层：macOS / Windows 系统接口。不依赖 Tauri。

pub mod error;
pub use error::PlatformError;

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
