//! 长截图重放（M23 规则 13）：example 与回归测试共用，保证两边的参数一致。
#![allow(dead_code)] // example 与测试各用一部分字段

use std::path::Path;
use std::time::Instant;
use xpaste_core::stitch::{Step, Stitch};

pub struct Replay {
    pub frames: usize,
    pub lost: usize,
    pub height: usize,
    pub slowest_ms: u128,
    /// 每帧 `push` 的平均耗时
    pub avg_ms: f64,
    /// `finish()` 的 (宽, 高, RGBA)
    pub image: (usize, usize, Vec<u8>),
}

/// 按文件名顺序重放目录里的 PNG，参数与应用一致：skip_right = round(24 × scale)，guard = round(40 × scale)。
/// `each` 收到每帧的结果文字（`=`、`+N`、`LOST`、`FULL`）。
pub fn replay(dir: &Path, scale: f64, mut each: impl FnMut(&str)) -> Replay {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("读不了帧目录")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "png"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "目录里没有 PNG 帧");
    let load = |p: &Path| {
        let im = image::open(p).expect("解码帧失败").into_rgba8();
        (im.width() as usize, im.height() as usize, im.into_raw())
    };
    let (w, h, first) = load(&files[0]);
    let mut st = Stitch::new(
        w,
        h,
        &first,
        (24.0 * scale).round() as usize,
        (40.0 * scale).round() as usize,
    );
    let (mut lost, mut slowest_ms, mut total) = (0, 0, 0.0);
    for p in &files[1..] {
        let (fw, fh, px) = load(p);
        assert_eq!((fw, fh), (w, h), "帧尺寸不一致");
        let t = Instant::now();
        let step = st.push(px);
        let e = t.elapsed();
        slowest_ms = slowest_ms.max(e.as_millis());
        total += e.as_secs_f64() * 1000.0;
        match step {
            Step::Same => each("="),
            Step::Added(n) => each(&format!("+{n}")),
            Step::Lost => {
                lost += 1;
                each("LOST")
            }
            Step::Full => each("FULL"),
        }
    }
    let height = st.height();
    Replay {
        frames: files.len(),
        lost,
        height,
        slowest_ms,
        avg_ms: total / (files.len() - 1).max(1) as f64,
        image: st.finish(),
    }
}
