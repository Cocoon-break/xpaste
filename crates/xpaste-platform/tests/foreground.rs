#![cfg(target_os = "macos")]
//! 前景分割抠主体：macOS 14+ 才有，更低版本跳过。

use image::{ImageFormat, Rgba, RgbaImage};
use std::io::Cursor;

#[test]
fn cutout_finds_subject() {
    if !xpaste_platform::foreground_supported() {
        return;
    }
    // 600×600 白底上一个实心红色圆
    let mut img = RgbaImage::from_pixel(600, 600, Rgba([255, 255, 255, 255]));
    for y in 0..600i32 {
        for x in 0..600i32 {
            if (x - 300).pow(2) + (y - 300).pow(2) <= 150 * 150 {
                img.put_pixel(x as u32, y as u32, Rgba([220, 30, 30, 255]));
            }
        }
    }
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .unwrap();
    let (w, h, rgba) = xpaste_platform::foreground_cutout(&png).expect("应抠到主体");
    assert_eq!((w, h, rgba.len()), (600, 600, 600 * 600 * 4));
    let alpha = |x: usize, y: usize| rgba[(y * 600 + x) * 4 + 3];
    assert_eq!(alpha(300, 300), 255);
    for (x, y) in [(0, 0), (599, 0), (0, 599), (599, 599)] {
        assert_eq!(alpha(x, y), 0);
    }
}
