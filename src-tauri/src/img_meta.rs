//! 图片编码和元数据：改字的成品写明用 xPaste 改过，含 AI 生成内容的另按 IPTC 注明。用 XMP，PNG、JPEG 都带。

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder, ImageResult};

/// 快速压缩档：大图默认档在 release 下也要近 2 秒（2818 × 3872），快速档不到 0.1 秒，文件大 2%–55%。
/// 像素照写，全透明处的 RGB 也在
pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> ImageResult<Vec<u8>> {
    let mut png = Vec::new();
    PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Adaptive)
        .write_image(rgba, w, h, ExtendedColorType::Rgba8)?;
    Ok(png)
}

/// 质量 95。`image` 的编码器三个分量都不降采样（4:4:4）
pub fn encode_jpeg(w: u32, h: u32, rgba: &[u8]) -> ImageResult<Vec<u8>> {
    let rgb: Vec<u8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let mut jpg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpg, 95).encode(&rgb, w, h, ExtendedColorType::Rgb8)?;
    Ok(jpg)
}

/// `ai`：图里有 AI 出的
fn xmp(ai: bool) -> String {
    let ai = if ai {
        r#" xmlns:Iptc4xmpExt="http://iptc.org/std/Iptc4xmpExt/2008-02-29/" Iptc4xmpExt:DigitalSourceType="http://cv.iptc.org/newscodes/digitalsourcetype/compositeWithTrainedAlgorithmicMedia""#
    } else {
        ""
    };
    format!(
        concat!(
            "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>",
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">"#,
            r#"<rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:CreatorTool="xPaste 改字"{}/>"#,
            r#"</rdf:RDF></x:xmpmeta><?xpacket end="r"?>"#
        ),
        ai
    )
}

/// 在 IHDR 后面加一块 iTXt。只给自己编码的 PNG 用
pub fn mark_png(mut png: Vec<u8>, ai: bool) -> Vec<u8> {
    const IHDR_END: usize = 8 + 12 + 13;
    // 关键字；不压缩；语言、译名都空
    let mut body = b"iTXtXML:com.adobe.xmp\0\0\0\0\0".to_vec();
    body.extend(xmp(ai).as_bytes());
    let mut chunk = (body.len() as u32 - 4).to_be_bytes().to_vec();
    let crc = crc32(&body);
    chunk.extend(body);
    chunk.extend(crc.to_be_bytes());
    png.splice(IHDR_END..IHDR_END, chunk);
    png
}

/// 在 SOI 后面加一段 APP1。只给自己编码的 JPEG 用
pub fn mark_jpeg(mut jpg: Vec<u8>, ai: bool) -> Vec<u8> {
    let mut body = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
    body.extend(xmp(ai).as_bytes());
    let mut seg = vec![0xFF, 0xE1];
    // 段长含自己两个字节，内容不到 1 KB
    seg.extend((body.len() as u16 + 2).to_be_bytes());
    seg.extend(body);
    jpg.splice(2..2, seg);
    jpg
}

/// PNG 用的 CRC-32
fn crc32(b: &[u8]) -> u32 {
    !b.iter().fold(!0u32, |c, &b| {
        (0..8).fold(c ^ u32::from(b), |c, _| {
            (c >> 1) ^ (0xEDB8_8320 & (c & 1).wrapping_neg())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOOL: &str = r#"xmp:CreatorTool="xPaste 改字""#;
    const AI: &str = r#"Iptc4xmpExt:DigitalSourceType="http://cv.iptc.org/newscodes/digitalsourcetype/compositeWithTrainedAlgorithmicMedia""#;

    fn rgba(w: u32, h: u32) -> Vec<u8> {
        (0..w * h * 4).map(|i| (i * 37 % 251) as u8).collect()
    }

    /// 依次的块：类型、数据、CRC
    fn chunks(png: &[u8]) -> Vec<([u8; 4], &[u8], u32)> {
        let u32_at = |i: usize| u32::from_be_bytes(png[i..i + 4].try_into().unwrap());
        let (mut at, mut out) = (8, Vec::new());
        while at < png.len() {
            let len = u32_at(at) as usize;
            let data = &png[at + 8..at + 8 + len];
            out.push((
                png[at + 4..at + 8].try_into().unwrap(),
                data,
                u32_at(at + 8 + len),
            ));
            at += 12 + len;
        }
        out
    }

    /// CRC 的标准校验值，和 PNG 里固定不变的 IEND
    #[test]
    fn crc_known() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn png_meta() {
        let px = rgba(5, 3);
        let plain = encode_png(5, 3, &px).unwrap();
        for ai in [false, true] {
            let png = mark_png(plain.clone(), ai);
            let c = chunks(&png);
            assert_eq!(&c[0].0, b"IHDR");
            let (ty, data, crc) = c[1];
            assert_eq!(&ty, b"iTXt");
            assert_eq!(crc, crc32(&[&ty, data].concat()));
            // 关键字、不压缩、语言和译名都空
            let xmp = data.strip_prefix(b"XML:com.adobe.xmp\0\0\0\0\0").unwrap();
            let xmp = std::str::from_utf8(xmp).unwrap();
            assert!(xmp.contains(TOOL));
            assert_eq!(xmp.contains(AI), ai);
            assert_eq!(xmp.contains("DigitalSourceType"), ai);
            // 只多这一块，其余照旧；能解，像素不变
            assert_eq!(c.len(), chunks(&plain).len() + 1);
            let back = image::load_from_memory(&png).unwrap();
            assert_eq!(back.color(), image::ColorType::Rgba8);
            assert!(back.into_rgba8().into_raw() == px);
        }
    }

    #[test]
    fn jpeg_meta() {
        let mut plain = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut plain, 95)
            .encode(&[90; 8 * 8 * 3], 8, 8, image::ExtendedColorType::Rgb8)
            .unwrap();
        for ai in [false, true] {
            let jpg = mark_jpeg(plain.clone(), ai);
            assert_eq!(jpg[..4], [0xFF, 0xD8, 0xFF, 0xE1]);
            let len = u16::from_be_bytes([jpg[4], jpg[5]]) as usize;
            let xmp = jpg[6..4 + len]
                .strip_prefix(b"http://ns.adobe.com/xap/1.0/\0")
                .unwrap();
            let xmp = std::str::from_utf8(xmp).unwrap();
            assert!(xmp.contains(TOOL));
            assert_eq!(xmp.contains(AI), ai);
            assert_eq!(xmp.contains("DigitalSourceType"), ai);
            // 后面原样；能解，像素不变
            assert_eq!(jpg[4 + len..], plain[2..]);
            let px = |b: &[u8]| image::load_from_memory(b).unwrap().into_rgb8().into_raw();
            assert_eq!(px(&jpg), px(&plain));
        }
    }
}
