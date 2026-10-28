use xpaste_platform::{freeze_displays, screen_capture_granted, window_rects};

#[test]
fn freeze_returns_tight_rgba() {
    if !screen_capture_granted() {
        eprintln!("跳过：无屏幕录制权限");
        return;
    }
    let frames = freeze_displays().unwrap();
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
