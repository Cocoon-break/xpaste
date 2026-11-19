use std::path::Path;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(pro)");
    // 只监听存在的路径：监听不存在的路径会让 cargo 每次都重跑本脚本、重编下游
    if Path::new("../../pro/rust/mod.rs").exists() {
        println!("cargo::rustc-cfg=pro");
        println!("cargo::rerun-if-changed=../../pro/rust");
    } else if Path::new("../../pro").exists() {
        println!("cargo::rerun-if-changed=../../pro");
    } else {
        println!("cargo::rerun-if-changed=build.rs");
    }
}
