//! Windows 平台层的纯函数：格式名映射、读取筛选、CF_HTML / CF_HDROP / DIB 解码、版本资源查询路径、托盘窗口类名、识别结果按词拼行。
//! 与 `decode.rs` 一样全平台编译，`windows.rs` 只负责 Win32 调用。

use std::borrow::Cow;
use std::io::Cursor;
use std::path::PathBuf;

use image::codecs::bmp::BmpDecoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageFormat, RgbaImage};
use xpaste_core::capture::MAX_FLAVOR_BYTES;
use xpaste_core::{Flavor, VideoMeta};

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

/// 头部声称解码后超过这个字节数（读取上限的 4 倍，200 MiB）就不解码：
/// `from_decoder` 按头部一次性分配缓冲区，伪造的 65535×65535 头会要十几 GB。
/// 不超过 50 MB 读取上限的未压缩 DIB 解码后最多约 67 MiB，只有 RLE 压缩的头会被这条拦下。
const MAX_DIB_DECODED_BYTES: u64 = 4 * MAX_FLAVOR_BYTES as u64;

/// 写回剪切板前的字节变换：`CF_UNICODETEXT`（13）补结尾 NUL（读取时去掉了，存储的数据不带 NUL，
/// 粘贴方按 NUL 找结尾）；其他格式空数据写 1 个 0 字节（`GlobalAlloc(GMEM_MOVEABLE, 0)` 的块锁不住，
/// 标记类格式只看存在与否）；其余原样。
pub fn write_bytes(id: u32, data: &[u8]) -> Cow<'_, [u8]> {
    match (id, data) {
        (13, _) => Cow::Owned([data, &[0, 0]].concat()),
        (_, []) => Cow::Borrowed(&[0]),
        _ => Cow::Borrowed(data),
    }
}

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
    encode_png(bgra_image(w, h, bgra)?)
}

/// 同 `bgra_to_png`，长边超过 `max_px` 时按比例缩小到不超过 `max_px`。
pub fn bgra_to_png_within(w: u32, h: u32, bgra: &[u8], max_px: u32) -> Option<Vec<u8>> {
    let img = opaque_if_no_alpha(bgra_image(w, h, bgra)?);
    if w.max(h) <= max_px {
        return encode_png(img);
    }
    let max = max_px.max(1);
    encode_png(
        DynamicImage::ImageRgba8(img)
            .resize(max, max, FilterType::Triangle)
            .to_rgba8(),
    )
}

fn bgra_image(w: u32, h: u32, bgra: &[u8]) -> Option<RgbaImage> {
    if (w as usize).checked_mul(h as usize)?.checked_mul(4)? != bgra.len() {
        return None;
    }
    let rgba = bgra
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[b, g, r, a]| [r, g, b, a])
        .collect();
    RgbaImage::from_raw(w, h, rgba)
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

/// 视频文件属性库（`IPropertyStore`）里读到的原始值，读不到的为 `None`。
#[derive(Debug, Clone, Copy, Default)]
pub struct VideoProps<'a> {
    /// `System.Media.Duration`，100 ns 单位
    pub duration_100ns: Option<u64>,
    /// `System.Video.FrameWidth` / `FrameHeight`
    pub w: Option<u32>,
    pub h: Option<u32>,
    /// `System.Video.FrameRate`，每千秒的帧数
    pub frame_rate_milli: Option<u32>,
    /// `System.Video.Orientation`，度
    pub orientation: Option<u32>,
    /// `System.Video.Compression`：子类型 GUID 字符串或编码名
    pub compression: Option<&'a str>,
    /// `System.Video.FourCC`
    pub fourcc: Option<u32>,
}

/// 属性值换算成 `VideoMeta`：时长换成秒，帧率换成帧（两位小数，缺失记 0），
/// 方向为 90° / 270° 时交换宽高，编码经 `codec_label`（`Compression` 认不出时用 `FourCC`，都没有记空串）。没有宽或高（或为 0）视为没有视频轨，返回 `None`。
pub fn video_meta_from_props(p: &VideoProps) -> Option<VideoMeta> {
    let (w, h) = (p.w.filter(|&v| v > 0)?, p.h.filter(|&v| v > 0)?);
    let (w, h) = if matches!(p.orientation, Some(90 | 270)) {
        (h, w)
    } else {
        (w, h)
    };
    let code = p
        .compression
        .map(subtype_fourcc)
        .filter(|s| !s.is_empty())
        .or_else(|| p.fourcc.map(fourcc_text))
        .unwrap_or_default();
    Some(VideoMeta {
        duration_sec: p.duration_100ns.map_or(0.0, |d| d as f64 / 1e7),
        w,
        h,
        fps: p
            .frame_rate_milli
            .map_or(0.0, |f| (f64::from(f) / 10.0).round() / 100.0),
        codec: crate::codec_label(&code),
    })
}

/// FourCC 子类型 GUID（`{XXXXXXXX-0000-0010-8000-00AA00389B71}`）还原成 4 个字符；
/// 其他 GUID 返回空串，不是 GUID 的原样返回（去首尾空白）。
fn subtype_fourcc(s: &str) -> String {
    let s = s.trim();
    let Some(guid) = s.strip_prefix('{').and_then(|g| g.strip_suffix('}')) else {
        return s.to_owned();
    };
    match guid.to_ascii_uppercase().split_once('-') {
        Some((head, "0000-0010-8000-00AA00389B71")) if head.len() == 8 => {
            u32::from_str_radix(head, 16).map_or_else(|_| String::new(), fourcc_text)
        }
        _ => String::new(),
    }
}

/// FourCC 数值（小端，首字符在最低字节）转文本；含不可打印字符时返回空串。
fn fourcc_text(v: u32) -> String {
    let b = v.to_le_bytes();
    if b.iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
        String::from_utf8_lossy(&b).trim().to_owned()
    } else {
        String::new()
    }
}

/// 很多程序写 32 位 DIB 时 alpha 全填 0：全零视为不透明，否则图片整张透明。
fn opaque_if_no_alpha(mut img: RgbaImage) -> RgbaImage {
    if img.pixels().all(|p| p[3] == 0) {
        img.pixels_mut().for_each(|p| p[3] = 255);
    }
    img
}

fn encode_png(img: RgbaImage) -> Option<Vec<u8>> {
    let img = opaque_if_no_alpha(img);
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// 中日韩文字与标点：汉字、假名、谚文、CJK 标点与全角符号。
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{00B7}'                   // 间隔号
        | '\u{1100}'..='\u{11FF}' // 谚文字母
        | '\u{2010}'..='\u{206F}' // 中文常用的引号、省略号、破折号等通用标点
        | '\u{2E80}'..='\u{9FFF}' // 部首、CJK 标点、假名、注音、汉字
        | '\u{AC00}'..='\u{D7AF}' // 谚文音节
        | '\u{F900}'..='\u{FAFF}' // 兼容汉字
        | '\u{FE10}'..='\u{FE6F}' // 竖排、兼容与小写变体标点
        | '\u{FF00}'..='\u{FFEF}' // 全角与半角形式
        | '\u{20000}'..='\u{3FFFF}') // 扩展汉字
}

/// 把 `OcrLine.Words()` 拼成一行：相邻两字都是中日韩字符时直接相连，否则隔一个空格。
/// （`OcrLine.Text()` 在中文词之间也插空格。）
pub fn join_ocr_words(words: &[String]) -> String {
    let mut out = String::new();
    for w in words.iter().filter(|w| !w.is_empty()) {
        let cjk_pair =
            out.chars().next_back().is_some_and(is_cjk) && w.chars().next().is_some_and(is_cjk);
        if !out.is_empty() && !cjk_pair {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_ocr_words_keeps_chinese_punctuation_tight() {
        let w: Vec<String> = ["说", "“", "你好", "”", "…", "再见"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(join_ocr_words(&w), "说“你好”…再见");
    }
    #[test]
    fn ocr_words_join_without_spaces_between_cjk() {
        let j = |ws: &[&str]| join_ocr_words(&ws.iter().map(|w| w.to_string()).collect::<Vec<_>>());
        assert_eq!(j(&["季", "度", "预", "算"]), "季度预算");
        assert_eq!(j(&["Budget", "2026"]), "Budget 2026");
        assert_eq!(j(&["预算", "Budget", "2026"]), "预算 Budget 2026");
        assert_eq!(
            j(&["こんにちは", "世界", "，", "안녕"]),
            "こんにちは世界，안녕"
        );
        assert_eq!(j(&[]), "");
    }

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
    fn write_bytes_terminates_unicode_text() {
        assert_eq!(&*write_bytes(13, &[0x61, 0]), &[0x61, 0, 0, 0]);
        assert_eq!(&*write_bytes(13, &[]), &[0, 0]);
    }
    #[test]
    fn write_bytes_pads_empty_other_formats() {
        assert_eq!(&*write_bytes(49_000, &[]), &[0]);
        assert_eq!(&*write_bytes(15, &[1, 2, 3]), &[1, 2, 3]);
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

    #[test]
    fn video_meta_from_props_converts_units_and_codec() {
        let base = VideoProps {
            duration_100ns: Some(30_000_000),
            w: Some(640),
            h: Some(360),
            frame_rate_milli: Some(30_000),
            compression: Some("{34363248-0000-0010-8000-00AA00389B71}"),
            ..Default::default()
        };
        assert_eq!(
            video_meta_from_props(&base),
            Some(VideoMeta {
                duration_sec: 3.0,
                w: 640,
                h: 360,
                fps: 30.0,
                codec: "H.264".into(),
            })
        );
        // 竖拍：交换宽高；29.97 帧；小写 GUID
        let m = video_meta_from_props(&VideoProps {
            orientation: Some(90),
            frame_rate_milli: Some(29_970),
            compression: Some(" {31435648-0000-0010-8000-00aa00389b71} "),
            ..base
        })
        .unwrap();
        assert_eq!(
            (m.w, m.h, m.fps, m.codec.as_str()),
            (360, 640, 29.97, "HEVC")
        );
        // 没有 Compression 时用 FourCC；非 FourCC 子类型记空串；纯文本照常转换
        let codec = |compression, fourcc| {
            video_meta_from_props(&VideoProps {
                compression,
                fourcc,
                ..base
            })
            .unwrap()
            .codec
        };
        assert_eq!(codec(None, Some(u32::from_le_bytes(*b"WMV3"))), "WMV");
        assert_eq!(
            codec(Some("{00000016-0000-0010-8000-00AA00389B71}"), None),
            ""
        );
        assert_eq!(
            codec(Some("{e06d8026-db46-11cf-b4d1-00805f6cbbea}"), None),
            ""
        );
        let vp90 = Some(u32::from_le_bytes(*b"VP90"));
        assert_eq!(
            codec(Some("{e06d8026-db46-11cf-b4d1-00805f6cbbea}"), vp90),
            "VP9"
        );
        assert_eq!(codec(Some(""), vp90), "VP9");
        assert_eq!(codec(Some("MP4V"), None), "MPEG-4");
        assert_eq!(codec(None, None), "");
        // 时长、帧率缺失记 0；没有尺寸视为没有视频轨
        let m = video_meta_from_props(&VideoProps {
            duration_100ns: None,
            frame_rate_milli: None,
            ..base
        })
        .unwrap();
        assert_eq!((m.duration_sec, m.fps), (0.0, 0.0));
        assert_eq!(video_meta_from_props(&VideoProps { w: None, ..base }), None);
        assert_eq!(
            video_meta_from_props(&VideoProps { h: Some(0), ..base }),
            None
        );
    }

    #[test]
    fn bgra_to_png_within_caps_long_edge() {
        // 4 × 2 的红色，alpha 全 0（按不透明处理，缩放后颜色不能变黑）
        let px: Vec<u8> = [0, 0, 255, 0].repeat(8);
        let decode = |png: Vec<u8>| image::load_from_memory(&png).unwrap().to_rgba8();
        let small = decode(bgra_to_png_within(4, 2, &px, 2).unwrap());
        assert_eq!(small.dimensions(), (2, 1));
        assert_eq!(small.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(
            decode(bgra_to_png_within(4, 2, &px, 8).unwrap()).dimensions(),
            (4, 2)
        );
        assert_eq!(
            decode(bgra_to_png_within(4, 2, &px, 4).unwrap()).dimensions(),
            (4, 2)
        );
        assert!(bgra_to_png_within(4, 2, &px[4..], 8).is_none());
    }
}
