//! xPaste 平台层：macOS / Windows 系统接口。不依赖 Tauri。

pub mod decode;
pub mod error;
pub mod record;
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

/// 冻结的一块显示器。坐标为原生单位：macOS 全局点（主屏左上为原点），Windows 虚拟桌面物理像素。
/// `rgba` 非预乘、行紧密排列（`px.0 * px.1 * 4` 字节）。
pub struct Frame {
    /// macOS 为 `CGDirectDisplayID`（录屏按它找显示器），Windows 为显示器序号（主屏为 0）
    pub id: u32,
    pub origin: (f64, f64),
    pub size: (f64, f64),
    pub scale: f64,
    pub px: (u32, u32),
    pub rgba: Vec<u8>,
}

/// 窗口外框，单位同 `Frame`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WinRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 本进程窗口里只有贴图（标题以此开头）参与窗口吸附；遮罩（`xpaste-shot…`）与面板都不算。
const PIN_TITLE_PREFIX: &str = "xpaste-pin";

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

/// 识别图片文件里的文字：读文件后交给 `ocr_png_bytes`（两端各自实现）。阻塞，只在工作线程调用。
pub fn ocr_png(path: &Path, max_side_px: u32) -> Result<Option<OcrText>, OcrError> {
    let bytes = std::fs::read(path).map_err(|_| OcrError::Decode)?;
    ocr_png_bytes(&bytes, max_side_px)
}

/// 解码图片字节（按内容识别格式）并把长边缩到 `max_side` 以内（不放大），RGBA8。
fn load_rgba(bytes: &[u8], max_side: u32) -> Result<image::RgbaImage, OcrError> {
    let img = image::load_from_memory(bytes).map_err(|_| OcrError::Decode)?;
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

/// 系统翻译语言包状态（M26 规则 7）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackStatus {
    /// 已下载
    Installed,
    /// 支持，需要用户在系统设置里下载
    Download,
    /// 不支持（含系统低于 macOS 26、Windows）
    Unsupported,
}

/// 系统翻译失败原因（M26 规则 7、13）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SysTranslateError {
    /// 系统低于 macOS 26
    OsTooOld,
    /// 缺语言包；`source` 为识别出的源语言代码，用来拼提示文案
    NotInstalled { source: String },
    /// 语言不支持或识别不出源语言
    Unsupported,
    /// 30 秒内没有结果
    Timeout,
    /// 其他失败；不带消息，日志另记错误码
    Failed,
}

/// 每 `step_ms` 查一次 `done`，为真立即返回 true；累计等满 `max_ms` 后最后查一次仍为假返回 false。
pub(crate) fn wait_until(mut done: impl FnMut() -> bool, step_ms: u64, max_ms: u64) -> bool {
    let mut waited = 0;
    loop {
        if done() {
            return true;
        }
        if waited >= max_ms {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(step_ms));
        waited += step_ms;
    }
}

/// 模拟一次“复制”（macOS ⌘C，Windows Ctrl+Insert），返回剪切板有没有变（M26 规则 18）。
/// 先等修饰键松开（最多 500 ms，否则按快捷键的 ⌥⇧ 会叠进去），再发按键，最多等 400 ms 看剪切板序号。
/// macOS 没有辅助功能权限时返回 `NoAccessibility`，调用方改为翻译剪切板。
/// 阻塞最多约 900 ms，只在后台线程调用，不要在主线程上调。
pub fn copy_selection() -> Result<bool, PlatformError> {
    if can_paste(None) == PasteBlock::NeedAccessibility {
        return Err(PlatformError::NoAccessibility);
    }
    // 等不到松开就不发：Windows 会变成 Ctrl+Alt+Shift+Insert（Shift+Insert 在有的应用里是粘贴），
    // 注入的 Ctrl 松开还会把用户按着的 Ctrl 在逻辑上松掉。返回“没变”，调用方翻译剪切板
    if !wait_until(|| !modifiers_down(), 10, 500) {
        return Ok(false);
    }
    let before = change_count();
    send_copy()?;
    Ok(wait_until(|| change_count() != before, 20, 400))
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
    fn wait_until_stops_at_max() {
        let mut calls = 0;
        assert!(!super::wait_until(
            || {
                calls += 1;
                false
            },
            5,
            15
        ));
        // 0、5、10、15 ms 各查一次
        assert_eq!(calls, 4);
    }

    #[test]
    fn wait_until_returns_early() {
        let mut calls = 0;
        assert!(super::wait_until(
            || {
                calls += 1;
                calls == 2
            },
            5,
            1000
        ));
        assert_eq!(calls, 2);
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
