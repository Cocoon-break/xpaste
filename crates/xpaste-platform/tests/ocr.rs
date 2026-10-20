use std::path::Path;

use xpaste_platform::{OcrError, ocr_available, ocr_png};

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

#[test]
fn ocr_missing_file_is_decode_error() {
    assert!(matches!(
        ocr_png(Path::new("/nonexistent.png"), 2000),
        Err(OcrError::Decode)
    ));
}
