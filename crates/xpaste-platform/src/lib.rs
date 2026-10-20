//! xPaste 平台层：macOS / Windows 系统接口。不依赖 Tauri。

pub mod decode;
pub mod error;
pub mod winfmt;
pub use error::PlatformError;

use std::path::Path;

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
        "vp08" | "vp80" => "VP8",
        "vp09" | "vp90" => "VP9",
        "av01" => "AV1",
        "mp4v" => "MPEG-4",
        "jpeg" | "mjpg" | "mjpa" => "Motion JPEG",
        "wmv3" => "WMV",
        _ => return code.to_ascii_uppercase(),
    };
    label.into()
}

/// 图片文字识别结果：按阅读顺序以 `'\n'` 连接的行，`confidence` 为各行置信度均值（0–1）。
#[derive(Debug, Clone, PartialEq)]
pub struct OcrText {
    pub text: String,
    pub confidence: f32,
}

#[derive(Debug)]
pub enum OcrError {
    /// 文件不存在或解码失败
    Decode,
    /// 预留：超时由调用方线程计时
    Timeout,
    /// 引擎异常；消息不含路径
    Engine(String),
}

/// 读图（按内容识别格式）并把长边缩到 `max_side` 以内（不放大），RGBA8。
fn load_rgba(path: &Path, max_side: u32) -> Result<image::RgbaImage, OcrError> {
    let bytes = std::fs::read(path).map_err(|_| OcrError::Decode)?;
    let img = image::load_from_memory(&bytes).map_err(|_| OcrError::Decode)?;
    let img = if img.width().max(img.height()) > max_side {
        img.resize(max_side, max_side, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    Ok(img.to_rgba8())
}

/// 行文本与置信度拼成结果；没有行时置信度为 0。
fn ocr_text(lines: Vec<(String, f32)>) -> OcrText {
    let confidence = if lines.is_empty() {
        0.0
    } else {
        lines.iter().map(|l| l.1).sum::<f32>() / lines.len() as f32
    };
    let text = lines
        .into_iter()
        .map(|l| l.0)
        .collect::<Vec<_>>()
        .join("\n");
    OcrText { text, confidence }
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
    fn ocr_text_joins_lines_and_averages() {
        let r = super::ocr_text(vec![("a".into(), 0.5), ("b".into(), 1.0)]);
        assert_eq!((r.text.as_str(), r.confidence), ("a\nb", 0.75));
        let empty = super::ocr_text(vec![]);
        assert_eq!((empty.text.as_str(), empty.confidence), ("", 0.0));
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
        assert_eq!(codec_label("VP80"), "VP8");
        assert_eq!(codec_label("VP90"), "VP9");
        assert_eq!(codec_label(" xyz "), "XYZ");
        assert_eq!(codec_label(""), "");
    }
}
