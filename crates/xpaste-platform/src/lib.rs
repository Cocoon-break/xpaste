//! xPaste 平台层：macOS / Windows 系统接口。不依赖 Tauri。

pub mod decode;
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

/// 视频编码的显示名：macOS 传 FourCC（`avc1`），Windows 传子类型名（`H264`）。不区分大小写、去首尾空白；
/// 不认识的大写原样返回。
pub fn codec_label(fourcc: &str) -> String {
    let code = fourcc.trim().to_ascii_lowercase();
    let label = match code.as_str() {
        "avc1" | "avc3" | "h264" | "x264" => "H.264",
        "hvc1" | "hev1" | "hevc" | "h265" => "HEVC",
        "apcn" | "apch" | "apcs" | "apco" | "ap4h" | "ap4x" => "ProRes",
        "vp08" => "VP8",
        "vp09" => "VP9",
        "av01" => "AV1",
        "mp4v" => "MPEG-4",
        "jpeg" | "mjpg" | "mjpa" => "Motion JPEG",
        "wmv3" => "WMV",
        _ => return code.to_ascii_uppercase(),
    };
    label.into()
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

    #[test]
    fn codec_label_maps_fourcc_and_windows_names() {
        use super::codec_label;
        assert_eq!(codec_label("avc1"), "H.264");
        assert_eq!(codec_label("H264"), "H.264");
        assert_eq!(codec_label("hvc1"), "HEVC");
        assert_eq!(codec_label("hev1"), "HEVC");
        assert_eq!(codec_label("apch"), "ProRes");
        assert_eq!(codec_label("vp09"), "VP9");
        assert_eq!(codec_label(" xyz "), "XYZ");
        assert_eq!(codec_label(""), "");
    }
}
