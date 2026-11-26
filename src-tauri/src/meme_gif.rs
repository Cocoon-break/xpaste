//! 表情包的 GIF / PNG 编码。GIF 自己量化：alpha < 128 的像素算全透明，其余量化到给定颜色数，
//! 透明单独占一个调色板索引；每帧一个局部调色板，无限循环。

use std::borrow::Cow;
use std::io::Cursor;

/// `frames`：每帧 side × side × 4 的 RGBA。`delay_ms` 按 10 ms 取整，`colors` 含透明色那一个（2–256）。
pub fn encode_gif(frames: &[Vec<u8>], side: u32, delay_ms: u16, colors: u16) -> Vec<u8> {
    let opaque_colors = usize::from(colors.clamp(2, 256)) - 1;
    let mut out = Vec::new();
    {
        let mut enc =
            gif::Encoder::new(&mut out, side as u16, side as u16, &[]).expect("写进内存不会失败");
        enc.set_repeat(gif::Repeat::Infinite)
            .expect("写进内存不会失败");
        for rgba in frames {
            let px = rgba.as_chunks::<4>().0;
            let solid: Vec<u8> = px
                .iter()
                .filter(|p| p[3] >= 128)
                .flatten()
                .copied()
                .collect();
            // 全透明的帧也要一个调色板：拿一个黑色凑数
            let nq = color_quant::NeuQuant::new(
                10,
                opaque_colors,
                if solid.is_empty() {
                    &[0, 0, 0, 255]
                } else {
                    &solid
                },
            );
            let mut palette = nq.color_map_rgb();
            let clear = (palette.len() / 3) as u8;
            palette.extend([0, 0, 0]);
            let buffer: Vec<u8> = px
                .iter()
                .map(|p| {
                    if p[3] < 128 {
                        clear
                    } else {
                        nq.index_of(&[p[0], p[1], p[2], 255]) as u8
                    }
                })
                .collect();
            let frame = gif::Frame {
                width: side as u16,
                height: side as u16,
                delay: delay_ms.div_ceil(10),
                // 透明处不能透出上一帧
                dispose: gif::DisposalMethod::Background,
                transparent: Some(clear),
                palette: Some(palette),
                buffer: Cow::Owned(buffer),
                ..gif::Frame::default()
            };
            enc.write_frame(&frame).expect("写进内存不会失败");
        }
    }
    out
}

/// side × side 的 RGBA 编成 PNG（不动的成品）
pub fn encode_png(rgba: &[u8], side: u32) -> Vec<u8> {
    let mut out = Vec::new();
    image::RgbaImage::from_raw(side, side, rgba.to_vec())
        .expect("尺寸与 RGBA 字节数一致")
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .expect("写进内存不会失败");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(gif: &[u8]) -> (gif::Repeat, Vec<gif::Frame<'static>>) {
        let mut o = gif::DecodeOptions::new();
        o.set_color_output(gif::ColorOutput::Indexed);
        let mut d = o.read_info(gif).unwrap();
        let mut frames = Vec::new();
        while let Some(f) = d.read_next_frame().unwrap() {
            frames.push(f.clone());
        }
        (d.repeat(), frames)
    }

    #[test]
    fn gif_roundtrip() {
        let side = 240u32;
        let mut frames: Vec<Vec<u8>> = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]]
            .iter()
            .map(|c| c.repeat((side * side) as usize))
            .collect();
        // 第 2 帧左上角半透明（alpha=60）：要变成全透明
        frames[1][3] = 60;
        let out = encode_gif(&frames, side, 80, 256);
        let (repeat, got) = decode(&out);
        assert_eq!(got.len(), 3);
        assert_eq!(repeat, gif::Repeat::Infinite);
        assert!(got.iter().all(|f| f.delay == 8));
        let f = &got[1];
        assert_eq!(Some(f.buffer[0]), f.transparent);
        // 其余像素不是透明索引
        assert_ne!(Some(f.buffer[1]), f.transparent);
    }

    #[test]
    fn gif_colors_respected() {
        let side = 64u32;
        let mut x: u32 = 0x9E37_79B9;
        let frames: Vec<Vec<u8>> = (0..2)
            .map(|_| {
                (0..side * side)
                    .flat_map(|_| {
                        x ^= x << 13;
                        x ^= x >> 17;
                        x ^= x << 5;
                        let [r, g, b, _] = x.to_le_bytes();
                        [r, g, b, 255]
                    })
                    .collect()
            })
            .collect();
        let (_, got) = decode(&encode_gif(&frames, side, 80, 64));
        assert_eq!(got.len(), 2);
        for f in &got {
            assert!(f.palette.as_ref().unwrap().len() / 3 <= 64);
        }
    }

    #[test]
    fn png_roundtrip() {
        let rgba = [10u8, 20, 30, 40].repeat(4);
        let back = image::load_from_memory(&encode_png(&rgba, 2))
            .unwrap()
            .to_rgba8();
        assert_eq!(back.as_raw(), &rgba);
    }
}
