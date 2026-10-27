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

// Windows：每次调用都初始化又释放 COM；最后一次释放若拆掉整个 COM，windows crate 缓存的 OcrEngine
// 工厂就成了悬垂指针，下一次调用访问违例（CI 实测）。连续调用要能一直工作
#[test]
fn ocr_survives_repeated_calls() {
    for _ in 0..3 {
        let _ = ocr_available();
        let _ = ocr_png(Path::new("/nonexistent.png"), 2000);
    }
}

#[test]
fn ocr_missing_file_is_decode_error() {
    assert!(matches!(
        ocr_png(Path::new("/nonexistent.png"), 2000),
        Err(OcrError::Decode)
    ));
}
