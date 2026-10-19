//! 读快照 → `write` 写回 → 再读，比较 (item, format, 字节数) 集合是否一致。不打印内容。
//! 运行：`cargo run -p xpaste-platform --example roundtrip`

use std::collections::BTreeSet;

fn shape() -> (Vec<xpaste_core::Flavor>, BTreeSet<(u32, String, usize)>) {
    let s = xpaste_platform::read_snapshot()
        .expect("读取剪切板失败")
        .unwrap_or_default();
    let set = s
        .flavors
        .iter()
        .map(|f| (f.item, f.format.clone(), f.data.len()))
        .collect();
    (s.flavors, set)
}

fn main() {
    let (flavors, before) = shape();
    let seq = xpaste_platform::write(&flavors).expect("写回失败");
    let (_, after) = shape();
    println!(
        "flavors={} seq={seq} identical={}",
        before.len(),
        before == after
    );
    for d in before.symmetric_difference(&after) {
        println!("diff item={} format={} bytes={}", d.0, d.1, d.2);
    }
}
