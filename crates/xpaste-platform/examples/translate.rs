//! 手动验证系统翻译桥接：`cargo run -p xpaste-platform --example translate`
//!
//! macOS 上 Translation 框架要靠主队列回调，所以和应用里一样：在工作线程上调用，主线程跑 `dispatch_main`。

use std::time::Instant;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn dispatch_main() -> !;
}

fn main() {
    #[cfg(target_os = "macos")]
    {
        std::thread::spawn(run);
        // SAFETY: 主线程交给 libdispatch，进程由 run 结束
        unsafe { dispatch_main() }
    }
    #[cfg(not(target_os = "macos"))]
    run();
}

fn run() {
    println!(
        "translate_supported: {}",
        xpaste_platform::translate_supported()
    );
    println!(
        "translate_pack(zh-Hans): {:?}",
        xpaste_platform::translate_pack("zh-Hans")
    );
    let t0 = Instant::now();
    let r = xpaste_platform::translate_text(
        "The build failed because the lockfile is out of date.",
        "zh-Hans",
    );
    println!("translate_text: {r:?} ({} ms)", t0.elapsed().as_millis());
    std::process::exit(0);
}
