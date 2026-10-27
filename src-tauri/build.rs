fn main() {
    // tauri-build 默认把清单（依赖 Common Controls v6）随资源只链进可执行目标，`cargo test` 的测试程序
    // 拿不到：它引用了 comctl32 v6 才有的 TaskDialogIndirect，Windows 上以 STATUS_ENTRYPOINT_NOT_FOUND
    // 直接无法启动。改为关掉 tauri-build 的清单，自己把同样的清单链进所有目标（DLL 里的 ID 1 清单系统不读）
    let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run tauri-build");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=windows-app.rc");
        println!("cargo:rerun-if-changed=windows-app.manifest");
        embed_resource::compile_for_everything("windows-app.rc", embed_resource::NONE)
            .manifest_required()
            .unwrap();
    }
}
