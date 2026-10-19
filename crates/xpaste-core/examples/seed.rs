//! 测量用种子数据：`cargo run -p xpaste-core --release --example seed -- <dir> <n>`。
//! 目录必须不存在（不会写进已有数据目录）。记录不带 flavor，只用于搜索、启动与内存测量。

use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use xpaste_core::{Settings, Snapshot, Store, capture, detect};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(dir), Some(n)) = (args.next(), args.next()) else {
        return Err("用法：seed <目录> <条数>".into());
    };
    let dir = PathBuf::from(dir);
    let n: u64 = n.parse()?;
    std::fs::create_dir(&dir)
        .map_err(|e| format!("创建 {} 失败（目录已存在？）：{e}", dir.display()))?;

    let started = Instant::now();
    let mut store = Store::open(&dir)?;
    let settings = Settings {
        max_records: 5000,
        ..Default::default()
    };
    store.save_settings(&settings)?;
    store.kv_set("onboarded", "1")?;

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;
    for i in 0..n {
        let mut text = match i % 4 {
            0 => format!("xpaste 测量 {i:05} 文本"),
            1 => format!("https://example.com/xpaste/{i:05}"),
            2 => format!("fn seed_{i:05}() {{ let x = {i}; }}"),
            _ => format!("#{:06x}", i * 2654435761 % 0xFFFFFF),
        };
        if i % 10 == 0 {
            // "xpaste 测量 " 为 14 字节，146 次约 2 KB
            text.push_str(&"xpaste 测量 ".repeat(146));
        }
        let snapshot = Snapshot {
            text: Some(text),
            ..Default::default()
        };
        let kind = detect::kind(&snapshot);
        let now_ms = now - i as i64 * 120_000;
        let record = capture::build_record(snapshot, kind, now_ms);
        store.insert_or_merge(&record, now_ms, settings.max_records)?;
    }
    println!("已写入 {n} 条，用时 {} ms", started.elapsed().as_millis());
    Ok(())
}
