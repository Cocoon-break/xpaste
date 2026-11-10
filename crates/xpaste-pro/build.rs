use std::path::Path;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(pro)");
    println!("cargo::rerun-if-changed=../../pro/rust");
    // 私有实现在场就用它，否则用 stub
    if Path::new("../../pro/rust/mod.rs").exists() {
        println!("cargo::rustc-cfg=pro");
    }
}
