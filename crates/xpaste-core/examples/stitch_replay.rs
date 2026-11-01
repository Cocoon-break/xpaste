//! 长截图离线重放（M23 规则 13）：把一个目录里按文件名排序的 PNG 帧依次送进 `Stitch`，
//! 打印每帧的结果，用来在不开应用的情况下复现、调试拼接。
//!
//! 用法：`cargo run --release -p xpaste-core --example stitch_replay -- <帧目录> [--scale 2] [--out 长图.png]`
//!
//! 每帧一个结果：`=` 没有新内容，`+N` 长高 N 行，`LOST` 没接上，`FULL` 到上限。
//! 最后一行：`帧 N 没接上 L 高 H 最慢 Tms`。

use std::path::PathBuf;

#[path = "../tests/common/replay.rs"]
mod replay;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut dir = None;
    let mut scale = 2.0f64;
    let mut out = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--scale" => {
                scale = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--scale 需要数字")
            }
            "--out" => out = Some(PathBuf::from(args.next().expect("--out 需要路径"))),
            _ => dir = Some(PathBuf::from(a)),
        }
    }
    let dir = dir.expect("用法：stitch_replay <帧目录> [--scale 2] [--out 长图.png]");
    let r = replay::replay(&dir, scale, |s| print!("{s} "));
    println!();
    println!(
        "帧 {} 没接上 {} 高 {} 最慢 {}ms",
        r.frames, r.lost, r.height, r.slowest_ms
    );
    if let Some(p) = out {
        let (w, h, rgba) = r.image;
        image::save_buffer(p, &rgba, w as u32, h as u32, image::ColorType::Rgba8)
            .expect("写长图失败");
    }
}
