fn main() {
    // tauri-build 默认把清单（依赖 Common Controls v6）随资源只链进可执行目标，`cargo test` 的测试程序
    // 拿不到：它引用了 comctl32 v6 才有的 TaskDialogIndirect，Windows 上以 STATUS_ENTRYPOINT_NOT_FOUND
    // 直接无法启动。改为关掉 tauri-build 的清单，自己把同样的清单链进所有目标（DLL 里的 ID 1 清单系统不读）
    let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run tauri-build");
    // xpaste-platform 的 build.rs 里的 rustc-link-arg 只作用于它自己的目标，传不到这里的应用与测试程序：
    // 部署目标低于 12 时（dev 构建按 rustc 默认 11.0 链接）Swift 并发库按 @rpath 引用，没有这条 rpath 就启动不了；
    // Translation 显式弱链接，macOS 13–25 上照常启动（设计 §4.2）
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,Translation");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=windows-app.rc");
        println!("cargo:rerun-if-changed=windows-app.manifest");
        embed_resource::compile_for_everything("windows-app.rc", embed_resource::NONE)
            .manifest_required()
            .unwrap();
    }
}
