//! 录屏的平台无关部分：参数、结果与输出尺寸（M24）。录制本身在 `macos_record.rs`，Windows 只有桩。

use std::path::PathBuf;

pub use xpaste_core::RecQuality;

/// 输出视频长边上限（4K）。
pub const MAX_SIDE: u32 = 3840;

/// 一次录制的参数。
#[derive(Debug, Clone)]
pub struct RecordOptions {
    /// macOS 为 `CGDirectDisplayID`（即 `Frame.id`）
    pub display: u32,
    /// 显示器内的点：x, y, w, h
    pub rect: (f64, f64, f64, f64),
    /// 该显示器的缩放比（`Frame.scale`）；输出像素另按 `quality` 换算（`output_scale`）
    pub scale: f64,
    pub quality: RecQuality,
    /// 30 | 60
    pub fps: u32,
    pub system_audio: bool,
    /// macOS 15 起才录得到；更早的系统忽略
    pub microphone: bool,
    /// 不录进画面的窗口编号（`window_number`），通常是录屏边框
    pub exclude_window: Option<u32>,
    /// 输出的 MP4 路径
    pub path: PathBuf,
}

/// 停止录制的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordOutcome {
    Saved {
        duration_ms: u64,
        bytes: u64,
    },
    /// 一帧画面都没收到，没有留下文件
    Empty,
}

/// 麦克风授权状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicAccess {
    Granted,
    Denied,
    Undetermined,
}

/// 输出缩放（M24 规则 6）：高按屏幕实际像素（该屏缩放比），标准与小文件按逻辑尺寸（1）。
pub fn output_scale(quality: RecQuality, display_scale: f64) -> f64 {
    if quality == RecQuality::High {
        display_scale
    } else {
        1.0
    }
}

/// 小文件的平均码率（bps，M24 规则 6）：每像素每帧 0.05 bit，不低于 500 kbps（1440 × 900 @ 30 约 1.94 Mbps）。
pub fn small_bitrate(w: u32, h: u32, fps: u32) -> u32 {
    let bps = (f64::from(w) * f64::from(h) * f64::from(fps) * 0.05).round() as u32;
    bps.max(500_000)
}

/// 输出像素：选区逻辑尺寸 × 缩放比，长边不超过 `MAX_SIDE`（等比缩小），宽高向下取偶数，至少 2。
pub fn output_size(w: f64, h: f64, scale: f64) -> (u32, u32) {
    let (w, h) = (w * scale, h * scale);
    let long = w.max(h);
    let max = f64::from(MAX_SIDE);
    let k = (max / long).min(1.0);
    // 长边直接取上限，免得 v * k 的浮点误差落到 3839.99 再被取偶成 3838
    let side = |v: f64| if v >= long { v.min(max) } else { v * k };
    let even = |v: f64| ((v as u32) & !1).max(2); // as 对 NaN、负数饱和为 0
    (even(side(w)), even(side(h)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_size_scales_and_rounds_even() {
        assert_eq!(output_size(801.0, 451.0, 2.0), (1602, 902));
        assert_eq!(output_size(100.5, 33.3, 1.0), (100, 32));
    }

    #[test]
    fn output_size_caps_long_side_and_rounds_even() {
        assert_eq!(output_size(2560.0, 1440.0, 2.0), (3840, 2160));
        assert_eq!(output_size(1440.0, 2560.0, 2.0), (2160, 3840));
        let (w, h) = output_size(3008.0, 1692.0, 2.0);
        assert_eq!(w, MAX_SIDE);
        assert!(w % 2 == 0 && h % 2 == 0, "{w}x{h}");
    }

    #[test]
    fn output_scale_by_quality() {
        assert_eq!(output_scale(RecQuality::High, 2.0), 2.0);
        assert_eq!(output_scale(RecQuality::Standard, 2.0), 1.0);
        assert_eq!(output_scale(RecQuality::Small, 2.0), 1.0);
        assert_eq!(output_scale(RecQuality::High, 1.5), 1.5);
        // 标准仍受长边上限与取偶约束
        assert_eq!(
            output_size(5120.0, 2880.0, output_scale(RecQuality::Standard, 2.0)),
            (3840, 2160)
        );
        assert_eq!(
            output_size(801.0, 451.0, output_scale(RecQuality::Small, 2.0)),
            (800, 450)
        );
    }

    #[test]
    fn small_bitrate_scales_with_pixels_and_fps() {
        assert_eq!(small_bitrate(1440, 900, 30), 1_944_000);
        assert_eq!(small_bitrate(1440, 900, 60), 3_888_000);
        assert_eq!(small_bitrate(3840, 2160, 60), 24_883_200);
        // 小选区不低于下限
        assert_eq!(small_bitrate(320, 200, 30), 500_000);
        assert_eq!(small_bitrate(2, 2, 30), 500_000);
    }

    #[test]
    fn output_size_never_zero() {
        assert_eq!(output_size(0.4, 0.4, 1.0), (2, 2));
    }
}
