#![cfg(target_os = "macos")]

use std::thread;
use std::time::Duration;

use xpaste_platform::record::{RecordOptions, RecordOutcome, output_size};
use xpaste_platform::{freeze_displays, start_recording, video_meta};

fn options(name: &str) -> (RecordOptions, f64) {
    let main = &freeze_displays().unwrap()[0];
    let dir = std::env::temp_dir().join(format!("xpaste-record-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    let opts = RecordOptions {
        display: main.id,
        rect: (0.0, 0.0, 320.0, 200.0),
        scale: main.scale,
        system_audio: false,
        microphone: false,
        exclude_window: None,
        path,
    };
    (opts, main.scale)
}

#[test]
#[ignore = "需要屏幕录制权限"]
fn records_two_seconds() {
    let (opts, scale) = options("two.mp4");
    let path = opts.path.clone();
    let rec = start_recording(opts).unwrap();
    thread::sleep(Duration::from_secs(2));
    assert!(!rec.failed());
    let RecordOutcome::Saved { bytes, .. } = rec.stop().unwrap() else {
        panic!("2 s 录制不应为空");
    };
    assert!(bytes > 0);
    let meta = video_meta(&path).unwrap();
    assert!(
        (1.5..=3.0).contains(&meta.duration_sec),
        "{}",
        meta.duration_sec
    );
    assert_eq!((meta.w, meta.h), output_size(320.0, 200.0, scale));

    // 立即停止：可能一帧都没有
    let (opts, _) = options("instant.mp4");
    let path = opts.path.clone();
    match start_recording(opts).unwrap().stop().unwrap() {
        RecordOutcome::Empty => assert!(!path.exists()),
        RecordOutcome::Saved { .. } => assert!(path.exists()),
    }
}
