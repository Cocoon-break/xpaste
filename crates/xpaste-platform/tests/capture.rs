#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};
#[cfg(target_os = "macos")]
use xpaste_platform::{AreaOptions, WinRect, capture_rect, start_area_stream};
use xpaste_platform::{freeze_displays, screen_capture_granted, window_rects};

#[test]
fn freeze_returns_tight_rgba() {
    if !screen_capture_granted() {
        eprintln!("跳过：无屏幕录制权限");
        return;
    }
    // Windows 上权限恒为真，CI 的托管机器可能没有可截的桌面：CI 里冻结失败只跳过，本机照常严格
    let frames = match freeze_displays() {
        Err(e) if std::env::var_os("CI").is_some() => {
            eprintln!("跳过：CI 环境冻结屏幕失败：{e}");
            return;
        }
        r => r.unwrap(),
    };
    assert!(!frames.is_empty());
    for f in &frames {
        assert_eq!(f.rgba.len(), f.px.0 as usize * f.px.1 as usize * 4);
        assert!(f.scale >= 1.0, "scale={}", f.scale);
        // macOS 的 size 是点，Windows 的 size 就是物理像素
        let expect = if cfg!(target_os = "macos") {
            f.size.0 * f.scale
        } else {
            f.size.0
        };
        assert!(
            (f64::from(f.px.0) - expect).abs() <= 1.0,
            "px={:?} size={:?} scale={}",
            f.px,
            f.size,
            f.scale
        );
    }
}

#[test]
fn window_rects_have_positive_size() {
    for r in window_rects() {
        assert!(r.w > 0.0 && r.h > 0.0, "{r:?}");
    }
}

/// 区域画面流（M23 规则 13）与 `capture_rect` 对同一区域输出同样尺寸。
/// Windows 的原生画面流（任务 5）接上后再放开到两端。
#[cfg(target_os = "macos")]
#[test]
fn area_stream_matches_capture_rect() {
    if !screen_capture_granted() {
        eprintln!("跳过：无屏幕录制权限");
        return;
    }
    let main = freeze_displays().unwrap().remove(0);
    let rect = (100.0, 100.0, 200.0, 120.0);
    let native = WinRect {
        x: main.origin.0 + rect.0,
        y: main.origin.1 + rect.1,
        w: rect.2,
        h: rect.3,
    };
    let s = start_area_stream(AreaOptions {
        display: main.id,
        rect,
        scale: main.scale,
        exclude_window: None,
        native,
    })
    .unwrap();
    assert_eq!(s.mode(), "画面流");
    let f = s.next(Duration::from_secs(2)).expect("2 秒内应有一帧");
    let (cw, ch, crgba) = capture_rect(native, None).unwrap();
    assert_eq!((f.w, f.h), (cw, ch));
    assert_eq!(f.rgba.len(), f.w as usize * f.h as usize * 4);
    // 颜色只打印不断言：屏幕上可能有动的内容。两边各自转 sRGB，静止画面上实测最大差 3（差 ≤ 2 的占 99.8%）
    let diff = f.rgba.iter().zip(&crgba).map(|(a, b)| a.abs_diff(*b)).max();
    eprintln!("画面流与 capture_rect 逐通道最大差：{diff:?}");
    let t = Instant::now();
    s.stop();
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
}
