//! macOS 上把 `swift/translate.swift` 编成静态库并链接（M26 规则 7）。其他目标什么都不做，Windows 交叉 clippy 不受影响。

use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=swift/translate.swift");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64",
        other => other,
    }
    .to_owned();
    let out = env::var("OUT_DIR").unwrap();
    let sdk = xcrun(&["--show-sdk-path"]);
    let swiftc = xcrun(&["--find", "swiftc"]);
    // 部署目标 13.0 = 规格说明 1.4 的最低系统版本；Translation 用到的接口都在 #available(macOS 26) 后面
    let output = Command::new(&swiftc)
        .args(["-parse-as-library", "-emit-library", "-static", "-O"])
        .args(["-module-name", "XpTranslate"])
        .args(["-target", &format!("{arch}-apple-macos13.0")])
        .args(["-sdk", &sdk])
        .arg("-o")
        .arg(Path::new(&out).join("libxptranslate.a"))
        .arg("swift/translate.swift")
        .output()
        .expect("无法运行 swiftc");
    if !output.status.success() {
        panic!(
            "swiftc 编译 swift/translate.swift 失败（需要 Xcode 26 及以上的 SDK）：\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // swiftc 在 <toolchain>/usr/bin 下，往上两级是 toolchain
    let toolchain = Path::new(&swiftc)
        .ancestors()
        .nth(3)
        .unwrap()
        .display()
        .to_string();
    println!("cargo:rustc-link-search=native={out}");
    println!("cargo:rustc-link-lib=static=xptranslate");
    println!("cargo:rustc-link-search=native={sdk}/usr/lib/swift");
    println!("cargo:rustc-link-search=native={toolchain}/usr/lib/swift/macosx");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    println!("cargo:rustc-link-arg=-Wl,-weak_framework,Translation");
    println!("cargo:rustc-link-lib=framework=NaturalLanguage");
    // xp_tr_prepare 挂 SwiftUI 视图（NSHostingView）；两者 macOS 13 都有，正常链接
    println!("cargo:rustc-link-lib=framework=SwiftUI");
    println!("cargo:rustc-link-lib=framework=AppKit");
}

/// 跑 `xcrun` 取一行输出；失败时构建报错。
fn xcrun(args: &[&str]) -> String {
    let out = Command::new("xcrun")
        .args(args)
        .output()
        .expect("无法运行 xcrun");
    assert!(
        out.status.success(),
        "xcrun {args:?} 失败：{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
