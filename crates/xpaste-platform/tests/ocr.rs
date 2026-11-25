use std::path::Path;

use xpaste_platform::{OcrError, ocr_available, ocr_png, ocr_png_bytes};

#[test]
fn ocr_reads_fixture_text() {
    if !ocr_available() {
        eprintln!("skip: no OCR engine");
        return;
    }
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ocr-zh.png");
    let r = ocr_png(Path::new(fixture), 2000).unwrap().unwrap();
    assert!(r.text.contains("2026"), "{}", r.text); // 两端都能认数字
    if cfg!(target_os = "macos") {
        assert!(r.text.contains("预算"), "{}", r.text);
    }
    assert!(r.confidence > 0.0);
    // 缩小后仍可识别：长边 400 px
    let small = ocr_png(Path::new(fixture), 400).unwrap().unwrap();
    assert!(small.text.contains("2026"), "{}", small.text);
}

// Windows：每次调用都初始化又释放 COM；最后一次释放若拆掉整个 COM，windows crate 缓存的 OcrEngine
// 工厂就成了悬垂指针，下一次调用访问违例（CI 实测）。连续调用要能一直工作
#[test]
fn ocr_survives_repeated_calls() {
    for _ in 0..3 {
        let _ = ocr_available();
        let _ = ocr_png(Path::new("/nonexistent.png"), 2000);
        // 解码失败前已初始化 COM
        let _ = ocr_png_bytes(b"not a png", 2000);
    }
}

#[test]
fn ocr_missing_file_is_decode_error() {
    assert!(matches!(
        ocr_png(Path::new("/nonexistent.png"), 2000),
        Err(OcrError::Decode)
    ));
}

// 截图识别文字直接传 PNG 字节，不落临时文件（M23 规则 7）
#[test]
fn ocr_reads_png_bytes() {
    assert!(matches!(
        ocr_png_bytes(b"not a png", 2000),
        Err(OcrError::Decode)
    ));
    if !ocr_available() {
        eprintln!("skip: no OCR engine");
        return;
    }
    let png = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/ocr-zh.png"
    ))
    .unwrap();
    let r = ocr_png_bytes(&png, 8000).unwrap().unwrap();
    assert!(r.text.contains("2026"), "{}", r.text);
}

#[test]
fn ocr_scale_table() {
    assert_eq!(xpaste_platform::ocr_scale(274, 363), 3.0);
    assert_eq!(xpaste_platform::ocr_scale(500, 900), 2.0);
    assert_eq!(xpaste_platform::ocr_scale(1200, 1600), 1.0);
}

/// 黑字的墨迹外框（像素），当作“画字的矩形”。
#[cfg(target_os = "macos")]
fn ink_bbox(png: &[u8]) -> [f32; 4] {
    let img = image::load_from_memory(png).unwrap().to_luma8();
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in img.enumerate_pixels() {
        if p.0[0] < 128 {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    [x0 as f32, y0 as f32, x1 as f32, y1 as f32]
}

#[cfg(target_os = "macos")]
fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[cfg(target_os = "macos")]
#[test]
fn lines_have_quads() {
    let png = fixture("ocr-line.png");
    let lines = xpaste_platform::ocr_lines(&png, 8000).unwrap().unwrap();
    assert_eq!(lines.len(), 1);
    let l = &lines[0];
    assert_eq!(l.text, "会议改到三点");
    let [x0, y0, x1, y1] = ink_bbox(&png);
    // 四个角都落在画字矩形（墨迹外框）±6 px 内
    for (q, want) in l.quad.iter().zip([[x0, y0], [x1, y0], [x1, y1], [x0, y1]]) {
        assert!(
            (q[0] - want[0]).abs() <= 6.0 && (q[1] - want[1]).abs() <= 6.0,
            "{:?}",
            l.quad
        );
    }
    assert_eq!(l.chars.len(), 6);
    assert!(
        l.chars
            .windows(2)
            .all(|w| w[0][0] < w[1][0] && w[0][1] <= w[1][1])
    );
    assert!(
        l.chars
            .iter()
            .all(|c| c[0] >= 0.0 && c[1] <= 1.0 && c[0] < c[1])
    );
    assert!(
        l.chars[0][0] <= 0.1 && l.chars[5][1] >= 0.9,
        "{:?}",
        l.chars
    );
    // 相邻两字中心间距彼此相差不超过 35%
    let gaps: Vec<f32> = l
        .chars
        .windows(2)
        .map(|w| (w[1][0] + w[1][1] - w[0][0] - w[0][1]) / 2.0)
        .collect();
    let (lo, hi) = gaps
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &g| (a.min(g), b.max(g)));
    assert!(hi <= lo * 1.35, "{gaps:?}");
}

#[cfg(target_os = "macos")]
#[test]
fn tilted_line_quad() {
    let lines = xpaste_platform::ocr_lines(&fixture("ocr-line-tilt.png"), 8000)
        .unwrap()
        .unwrap();
    assert_eq!(lines.len(), 1);
    let q = lines[0].quad;
    let width = (q[1][0] - q[0][0]).hypot(q[1][1] - q[0][1]);
    assert!(q[1][1] - q[0][1] >= 0.15 * width, "{q:?}");
    // 上边长约等于画字宽度（原图水平字宽 / cos12°，±10%）
    let [x0, _, x1, _] = ink_bbox(&fixture("ocr-line.png"));
    let drawn = x1 - x0;
    assert!((width - drawn).abs() <= 0.1 * drawn, "{width} vs {drawn}");
    assert_eq!(lines[0].chars.len(), 6);
}
