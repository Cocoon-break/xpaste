//! 长截图自动滚动的平台接口（M23 规则 13）。在 macOS 与 Windows 上跑。
#![cfg(any(target_os = "macos", windows))]

use std::thread;
use std::time::Duration;

use xpaste_platform::{
    WinRect, can_auto_scroll, capture_rect, pointer_position, screen_capture_granted, scroll_at,
    watch_user_input,
};

/// 同应用（tao 设置的 Per-Monitor-v2）：Windows 上 `capture_rect` 与 `GetCursorPos` 才是物理像素
fn per_monitor_dpi() {
    #[cfg(windows)]
    // SAFETY: 只改本测试进程的 DPI 感知，无指针参数；已设置过时返回错误，忽略
    let _ = unsafe {
        windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
    };
}

/// 只读查询，不发事件。
#[test]
fn pointer_and_input_age_are_readable() {
    per_monitor_dpi();
    let _ = watch_user_input().age();
    // CI 的托管机器可能没有交互式桌面，取不到指针：CI 里只打印，本机照常严格
    if pointer_position().is_none() && std::env::var_os("CI").is_some() {
        eprintln!("跳过：CI 环境取不到指针位置");
        return;
    }
    assert!(pointer_position().is_some());
}

/// 会向主屏左上 (200, 200)–(600, 500) 处的窗口发真实滚动，只在本机手动跑：
/// 前台放一个铺满这块区域、能往下滚的窗口（如终端），跑的时候别碰键盘鼠标，
/// `cargo test -p xpaste-platform --test scroll -- --ignored`。
/// Windows 上坐标是物理像素，滚轮作用于指针下的窗口：先把指针停在这块区域里的那个窗口上。
#[test]
#[ignore = "向前台窗口发真实滚动，只手动跑"]
fn scroll_at_moves_content() {
    per_monitor_dpi();
    assert!(can_auto_scroll(), "需要辅助功能权限");
    assert!(screen_capture_granted(), "需要屏幕录制权限");
    let r = WinRect {
        x: 200.0,
        y: 200.0,
        w: 400.0,
        h: 300.0,
    };
    let watch = watch_user_input();
    let before = capture_rect(r, None).unwrap();
    let age0 = watch.age();
    let p0 = pointer_position();
    scroll_at(r.x + r.w / 2.0, r.y + r.h / 2.0, 200).unwrap();
    thread::sleep(Duration::from_millis(200));
    let after = capture_rect(r, None).unwrap();
    let age1 = watch.age();
    // 设计反目标：发滚动不移动用户的指针
    assert_eq!(p0, pointer_position(), "发滚动后指针动了");
    assert_ne!(before.2, after.2, "滚动后画面没变");
    // 本进程的滚动不算用户输入：距最近一次用户输入的时间只会变长
    if let Some(a1) = age1 {
        assert!(
            age0.is_some_and(|a0| a1 >= a0),
            "age0={age0:?} age1={age1:?}"
        );
    }
}
