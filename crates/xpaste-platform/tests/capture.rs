use std::time::{Duration, Instant};
use xpaste_platform::{
    AreaOptions, WinRect, capture_rect, freeze_displays, screen_capture_granted, start_area_stream,
    window_rects,
};

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
/// Windows：build 20348 起走画面流，更早的 Windows 10 回退逐帧截屏。
#[test]
fn area_stream_matches_capture_rect() {
    if !screen_capture_granted() {
        eprintln!("跳过：无屏幕录制权限");
        return;
    }
    // 同应用（tao 设置的 Per-Monitor-v2）：否则缩放屏上显示器矩形是逻辑尺寸，画面流会因对不上而回退
    #[cfg(windows)]
    // SAFETY: 只改本测试进程的 DPI 感知，无指针参数；已设置过时返回错误，忽略
    let _ = unsafe {
        windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
    };
    // CI 的托管机器可能没有可截的桌面或 GPU：CI 里截不到只跳过、模式不符只打印，本机照常严格
    let ci = std::env::var_os("CI").is_some();
    let main = match freeze_displays() {
        Err(e) if ci => {
            eprintln!("跳过：CI 环境冻结屏幕失败：{e}");
            return;
        }
        r => r.unwrap().remove(0),
    };
    let rect = (100.0, 100.0, 200.0, 120.0);
    // 同应用层 pin_bounds：macOS 原生单位是点，Windows 是物理像素
    let k = if cfg!(target_os = "macos") {
        1.0
    } else {
        main.scale
    };
    let native = WinRect {
        x: main.origin.0 + rect.0 * k,
        y: main.origin.1 + rect.1 * k,
        w: rect.2 * k,
        h: rect.3 * k,
    };
    let s = start_area_stream(AreaOptions {
        display: main.id,
        rect,
        scale: main.scale,
        exclude_window: None,
        native,
    })
    .unwrap();
    #[cfg(windows)]
    let want = if xpaste_platform::windows_build() >= 20348 {
        "画面流"
    } else {
        "逐帧"
    };
    #[cfg(not(windows))]
    let want = "画面流";
    eprintln!("模式：{}", s.mode());
    if ci && s.mode() != want {
        eprintln!("CI 环境模式为 {}（期望 {want}），不断言", s.mode());
    } else {
        assert_eq!(s.mode(), want);
    }
    let f = match s.next(Duration::from_secs(2)) {
        None if ci => {
            eprintln!("跳过：CI 环境 2 秒内没有帧");
            return;
        }
        f => f.expect("2 秒内应有一帧"),
    };
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
