//! Windows 平台层的纯函数：格式名映射、读取筛选、CF_HTML / CF_HDROP / DIB 解码、版本资源查询路径、托盘窗口类名。
//! 与 `decode.rs` 一样全平台编译，`windows.rs` 只负责 Win32 调用。

use std::io::Cursor;
use std::path::PathBuf;

use image::codecs::bmp::BmpDecoder;
use image::{DynamicImage, ImageDecoder, ImageFormat, RgbaImage};
use xpaste_core::Flavor;

/// 系统预定义格式（`CF_*`）：id 与名字。`GetClipboardFormatNameW` 取不到这些名字。
pub const STANDARD_FORMATS: [(u32, &str); 22] = [
    (1, "CF_TEXT"),
    (2, "CF_BITMAP"),
    (3, "CF_METAFILEPICT"),
    (4, "CF_SYLK"),
    (5, "CF_DIF"),
    (6, "CF_TIFF"),
    (7, "CF_OEMTEXT"),
    (8, "CF_DIB"),
    (9, "CF_PALETTE"),
    (10, "CF_PENDATA"),
    (11, "CF_RIFF"),
    (12, "CF_WAVE"),
    (13, "CF_UNICODETEXT"),
    (14, "CF_ENHMETAFILE"),
    (15, "CF_HDROP"),
    (16, "CF_LOCALE"),
    (17, "CF_DIBV5"),
    (0x80, "CF_OWNERDISPLAY"),
    (0x81, "CF_DSPTEXT"),
    (0x82, "CF_DSPBITMAP"),
    (0x83, "CF_DSPMETAFILEPICT"),
    (0x8E, "CF_DSPENHMETAFILE"),
];

pub fn standard_format_name(id: u32) -> Option<&'static str> {
    STANDARD_FORMATS.iter().find(|f| f.0 == id).map(|f| f.1)
}

pub fn standard_format_id(name: &str) -> Option<u32> {
    STANDARD_FORMATS.iter().find(|f| f.1 == name).map(|f| f.0)
}

/// 要读取内容的格式（保持顺序）：去掉 GDI 句柄格式、系统由 `CF_UNICODETEXT` 合成的
/// `CF_TEXT` / `CF_OEMTEXT`、OLE 内部格式；`CF_DIB` / `CF_DIBV5` 只留先出现的（后者是系统合成的）。
pub fn formats_to_read(all: &[(u32, String)]) -> Vec<(u32, String)> {
    const SKIP_IDS: [u32; 10] = [1, 2, 3, 7, 9, 14, 0x80, 0x82, 0x83, 0x8E];
    let mut dib_seen = false;
    all.iter()
        .filter(|(id, name)| {
            if SKIP_IDS.contains(id) || matches!(name.as_str(), "DataObject" | "Ole Private Data") {
                return false;
            }
            if matches!(id, 8 | 17) {
                return !std::mem::replace(&mut dib_seen, true);
            }
            true
        })
        .cloned()
        .collect()
}

/// `HTML Format`（CF_HTML）：按头部 `StartFragment:` / `EndFragment:` 的字节偏移取出片段。
pub fn cf_html_fragment(bytes: &[u8]) -> Option<String> {
    let offset = |key: &[u8]| -> Option<usize> {
        let at = bytes.windows(key.len()).position(|w| w == key)? + key.len();
        let digits = bytes[at..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        std::str::from_utf8(&bytes[at..at + digits])
            .ok()?
            .parse()
            .ok()
    };
    let start = offset(b"StartFragment:")?;
    let end = offset(b"EndFragment:")?;
    let fragment = bytes.get(start..end)?;
    Some(String::from_utf8_lossy(fragment).into_owned())
}

/// `CF_HDROP`（DROPFILES）：`pFiles` 处起逐个 NUL 结尾的路径，空串结束；`fWide` 决定 UTF-16LE 还是单字节。
pub fn parse_hdrop(bytes: &[u8]) -> Vec<PathBuf> {
    let dword = |at: usize| {
        bytes
            .get(at..at + 4)
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let Some(list) = bytes.get(dword(0) as usize..) else {
        return Vec::new();
    };
    let names: Vec<String> = if dword(16) != 0 {
        let wide: Vec<u16> = list
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .collect();
        wide.split(|&c| c == 0)
            .take_while(|s| !s.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    } else {
        list.split(|&b| b == 0)
            .take_while(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect()
    };
    names.into_iter().map(PathBuf::from).collect()
}

/// 头部声称的尺寸超过这个字节数就不解码（与 image 默认 reader 的上限一致）：
/// `from_decoder` 按头部一次性分配缓冲区，伪造的 65535×65535 头会要十几 GB。
const MAX_DIB_DECODED_BYTES: u64 = 512 << 20;

/// `CF_DIB` / `CF_DIBV5`（无文件头的 BMP）转 PNG。
pub fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    let decoder = BmpDecoder::new_without_file_header(Cursor::new(dib)).ok()?;
    if decoder.total_bytes() > MAX_DIB_DECODED_BYTES {
        return None;
    }
    encode_png(DynamicImage::from_decoder(decoder).ok()?.to_rgba8())
}

/// 自上而下的 BGRA 像素转 PNG；长度必须正好是 `w * h * 4`。
pub fn bgra_to_png(w: u32, h: u32, bgra: &[u8]) -> Option<Vec<u8>> {
    if (w as usize).checked_mul(h as usize)?.checked_mul(4)? != bgra.len() {
        return None;
    }
    let rgba = bgra
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[b, g, r, a]| [r, g, b, a])
        .collect();
    encode_png(RgbaImage::from_raw(w, h, rgba)?)
}

/// 已读出的 flavor 中有 `PNG` 时原样返回；否则第一个 `CF_DIBV5` / `CF_DIB` 转成 PNG。
pub fn bitmap_from_flavors(flavors: &[Flavor]) -> Option<Vec<u8>> {
    if let Some(png) = flavors.iter().find(|f| f.format == "PNG") {
        return Some(png.data.clone());
    }
    let dib = flavors
        .iter()
        .find(|f| matches!(f.format.as_str(), "CF_DIBV5" | "CF_DIB"))?;
    let png = dib_to_png(&dib.data);
    if png.is_none() {
        log::debug!("{} 转 PNG 失败 bytes={}", dib.format, dib.data.len());
    }
    png
}

/// 版本资源里 `FileDescription` 的查询路径。`translation` 是 `\VarFileInfo\Translation` 的值，
/// 前 4 字节为（语言 u16 LE，代码页 u16 LE）；不足 4 字节时用美国英语 + Unicode（`040904b0`）。
pub fn file_description_query(translation: &[u8]) -> String {
    let (lang, cp) = match translation {
        [a, b, c, d, ..] => (u16::from_le_bytes([*a, *b]), u16::from_le_bytes([*c, *d])),
        _ => (0x0409, 0x04b0),
    };
    format!(r"\StringFileInfo\{lang:04x}{cp:04x}\FileDescription")
}

/// 任务栏、副屏任务栏与通知区域溢出窗口的类名（区分大小写）：它们成为前台时不作粘贴目标。
pub fn is_shell_tray_class(class: &str) -> bool {
    matches!(
        class,
        "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "NotifyIconOverflowWindow"
            | "TopLevelWindowForOverflowXamlIsland"
    )
}

/// 很多程序写 32 位 DIB 时 alpha 全填 0：全零视为不透明，否则图片整张透明。
fn encode_png(mut img: RgbaImage) -> Option<Vec<u8>> {
    if img.pixels().all(|p| p[3] == 0) {
        img.pixels_mut().for_each(|p| p[3] = 255);
    }
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 40 字节 BITMAPINFOHEADER（BI_RGB）+ 像素
    fn dib(w: i32, h: i32, bpp: u16, px: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend(40u32.to_le_bytes());
        v.extend(w.to_le_bytes());
        v.extend(h.to_le_bytes());
        v.extend(1u16.to_le_bytes());
        v.extend(bpp.to_le_bytes());
        v.extend(0u32.to_le_bytes()); // BI_RGB
        v.extend((px.len() as u32).to_le_bytes());
        v.extend([0; 16]); // 分辨率、调色板计数
        v.extend(px);
        v
    }

    /// 20 字节 DROPFILES 头 + 逐个 NUL 结尾的路径 + 结尾空串
    fn dropfiles(wide: bool, names: &[&str]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend(20u32.to_le_bytes());
        v.extend([0; 12]); // pt、fNC
        v.extend(u32::from(wide).to_le_bytes());
        for name in names.iter().chain([&""]) {
            if wide {
                v.extend(name.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
            } else {
                v.extend(name.bytes().chain([0]));
            }
        }
        v
    }

    fn cf_html(fragment: &str) -> Vec<u8> {
        let header = |s: usize, e: usize| {
            format!("Version:0.9\r\nStartFragment:{s:010}\r\nEndFragment:{e:010}\r\n")
        };
        let prefix = "<html><body><!--StartFragment-->";
        let start = header(0, 0).len() + prefix.len();
        let end = start + fragment.len();
        format!(
            "{}{prefix}{fragment}<!--EndFragment--></body></html>",
            header(start, end)
        )
        .into_bytes()
    }

    #[test]
    fn standard_format_names_round_trip() {
        assert_eq!(standard_format_name(13), Some("CF_UNICODETEXT"));
        assert_eq!(standard_format_name(0xC0FE), None);
        assert_eq!(standard_format_id("CF_DIBV5"), Some(17));
        assert_eq!(standard_format_id("HTML Format"), None);
        for (id, name) in STANDARD_FORMATS {
            assert_eq!(standard_format_id(name), Some(id));
        }
    }
    #[test]
    fn formats_to_read_skips_gdi_synthesized_and_ole() {
        let all: Vec<(u32, String)> = [
            (1, "CF_TEXT"),
            (13, "CF_UNICODETEXT"),
            (16, "CF_LOCALE"),
            (7, "CF_OEMTEXT"),
            (17, "CF_DIBV5"),
            (2, "CF_BITMAP"),
            (8, "CF_DIB"),
            (14, "CF_ENHMETAFILE"),
            (3, "CF_METAFILEPICT"),
            (9, "CF_PALETTE"),
            (0xC013, "DataObject"),
            (0xC00E, "Ole Private Data"),
            (0xC0A1, "HTML Format"),
            (0xC0B2, "PNG"),
        ]
        .map(|(i, n)| (i, n.to_string()))
        .into();
        let ids: Vec<u32> = formats_to_read(&all).into_iter().map(|f| f.0).collect();
        assert_eq!(ids, [13, 16, 17, 0xC0A1, 0xC0B2]); // DIBV5 先出现，DIB 是合成的
    }
    #[test]
    fn cf_html_fragment_uses_byte_offsets() {
        let frag = r#"<a href="https://a.io">锚文本</a>"#;
        assert_eq!(cf_html_fragment(&cf_html(frag)).as_deref(), Some(frag));
        assert_eq!(cf_html_fragment(b"Version:0.9\r\n"), None);
        assert_eq!(
            cf_html_fragment(b"StartFragment:0000000010\r\nEndFragment:0000009999\r\n"),
            None
        );
    }
    #[test]
    fn parse_hdrop_wide_and_ansi() {
        assert_eq!(
            parse_hdrop(&dropfiles(true, &[r"C:\a b\图.png", r"D:\x"])),
            [PathBuf::from(r"C:\a b\图.png"), PathBuf::from(r"D:\x")]
        );
        assert_eq!(
            parse_hdrop(&dropfiles(false, &[r"C:\a"])),
            [PathBuf::from(r"C:\a")]
        );
        assert!(parse_hdrop(&[0xFF, 0, 0, 0]).is_empty());
    }
    #[test]
    fn dib_to_png_handles_bottom_up_and_zero_alpha() {
        let px = |png: &[u8], x, y| {
            image::load_from_memory(png)
                .unwrap()
                .to_rgba8()
                .get_pixel(x, y)
                .0
        };
        let png = dib_to_png(&dib(1, 2, 24, &[0, 0, 255, 0, 0, 255, 0, 0])).unwrap(); // 第一行在底部
        assert_eq!(
            (px(&png, 0, 1), px(&png, 0, 0)),
            ([255, 0, 0, 255], [0, 255, 0, 255])
        );
        let png = dib_to_png(&dib(2, 1, 32, &[0, 0, 255, 0, 0, 255, 0, 0])).unwrap();
        assert_eq!(px(&png, 1, 0), [0, 255, 0, 255]);
        assert_eq!(dib_to_png(&[1, 2, 3]), None);
    }
    #[test]
    fn dib_to_png_rejects_huge_header_before_allocating() {
        let huge = dib(65535, 65535, 32, &[]);
        // 头部本身合法，确保是字节上限而不是解析失败让它返回 None
        let decoder = BmpDecoder::new_without_file_header(Cursor::new(&huge)).unwrap();
        assert!(decoder.total_bytes() > MAX_DIB_DECODED_BYTES);
        assert_eq!(dib_to_png(&huge), None);
    }
    #[test]
    fn bgra_to_png_treats_all_zero_alpha_as_opaque() {
        let px = |png: Vec<u8>| {
            image::load_from_memory(&png)
                .unwrap()
                .to_rgba8()
                .get_pixel(0, 0)
                .0
        };
        assert_eq!(
            px(bgra_to_png(2, 1, &[0, 0, 255, 0, 0, 255, 0, 0]).unwrap()),
            [255, 0, 0, 255]
        );
        assert_eq!(
            px(bgra_to_png(1, 1, &[0, 0, 255, 128]).unwrap()),
            [255, 0, 0, 128]
        );
        assert_eq!(bgra_to_png(2, 2, &[0; 4]), None);
    }
    #[test]
    fn bitmap_prefers_png_flavor() {
        let f = |format: &str, data: Vec<u8>| Flavor {
            item: 0,
            format: format.into(),
            data,
        };
        let png = vec![0x89, b'P', b'N', b'G'];
        let red = dib(1, 1, 24, &[0, 0, 255, 0]);
        assert_eq!(
            bitmap_from_flavors(&[f("CF_DIB", red.clone()), f("PNG", png.clone())]),
            Some(png)
        );
        assert!(bitmap_from_flavors(&[f("CF_DIBV5", red)]).is_some());
        assert_eq!(bitmap_from_flavors(&[f("CF_UNICODETEXT", vec![])]), None);
    }
    #[test]
    fn file_description_query_from_translation() {
        assert_eq!(
            file_description_query(&[0x09, 0x04, 0xB0, 0x04]),
            r"\StringFileInfo\040904b0\FileDescription"
        );
        assert_eq!(
            file_description_query(&[0x04, 0x08, 0xA8, 0x03]),
            r"\StringFileInfo\080403a8\FileDescription"
        );
        assert_eq!(
            file_description_query(&[]),
            r"\StringFileInfo\040904b0\FileDescription"
        );
    }
    #[test]
    fn shell_tray_classes_are_not_paste_targets() {
        for c in [
            "Shell_TrayWnd",
            "Shell_SecondaryTrayWnd",
            "NotifyIconOverflowWindow",
            "TopLevelWindowForOverflowXamlIsland",
        ] {
            assert!(is_shell_tray_class(c), "{c}");
        }
        assert!(!is_shell_tray_class("Notepad"));
        assert!(!is_shell_tray_class("shell_traywnd")); // 类名区分大小写
    }
}
