//! 手动验证语言包下载（设计 §3.1）：`cargo run -p xpaste-platform --example translate_prepare [源] [目标]`，默认 `en` → `ko`。
//!
//! 和应用里一样：accessory 应用（不进程序坞）、一个不激活应用的 NSPanel 当“xPaste 窗口”，
//! 主线程跑事件循环，工作线程调 `translate_prepare`。会弹出系统的“下载翻译语言”确认框。

fn main() {
    #[cfg(target_os = "macos")]
    mac::run();
    #[cfg(not(target_os = "macos"))]
    println!("只在 macOS 上有：其他系统 translate_prepare 恒为 Unsupported");
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use std::time::Instant;

    pub fn run() {
        let mut args = std::env::args().skip(1);
        let source = args.next().unwrap_or_else(|| "en".into());
        let target = args.next().unwrap_or_else(|| "ko".into());
        let cls = |n: &std::ffi::CStr| AnyClass::get(n).unwrap();
        // SAFETY: 主线程上按 AppKit 的约定建应用和窗口；窗口一直不释放，进程结束为止
        let panel = unsafe {
            let app: *mut AnyObject = msg_send![cls(c"NSApplication"), sharedApplication];
            let _: bool = msg_send![app, setActivationPolicy: 1isize]; // accessory
            let p: *mut AnyObject = msg_send![cls(c"NSPanel"), alloc];
            let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(360.0, 120.0));
            // titled | nonactivatingPanel；backing buffered
            let p: *mut AnyObject = msg_send![p, initWithContentRect: rect, styleMask: 1usize | (1 << 7), backing: 2usize, defer: false];
            let title = NSString::from_str("xPaste 测试：下载语言包");
            let _: () = msg_send![p, setTitle: &*title];
            let _: () = msg_send![p, setLevel: 3isize]; // floating
            let _: () = msg_send![p, center];
            let _: () = msg_send![p, orderFrontRegardless];
            p as usize
        };
        std::thread::spawn(move || {
            // translate_pack 也靠主队列，要在工作线程查
            println!(
                "translate_pack({target}) 下载前: {:?}",
                xpaste_platform::translate_pack(&target)
            );
            let t0 = Instant::now();
            // SAFETY: panel 从不释放
            let r =
                unsafe { xpaste_platform::translate_prepare(&source, &target, panel as *mut _) };
            println!(
                "translate_prepare({source}, {target}): {r:?} ({} ms)",
                t0.elapsed().as_millis()
            );
            std::process::exit(0);
        });
        // SAFETY: 主线程跑事件循环，进程由上面的线程结束
        unsafe {
            let app: *mut AnyObject = msg_send![cls(c"NSApplication"), sharedApplication];
            let _: () = msg_send![app, run];
        }
    }
}
