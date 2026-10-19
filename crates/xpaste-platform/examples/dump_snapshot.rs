//! 打印当前剪切板快照的结构（格式名、字节数、各字段有无），不打印内容。
//! 运行：`cargo run -p xpaste-platform --example dump_snapshot [-- --icon <bundle id 或 exe 路径>]`

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let [_, flag, id] = args.as_slice()
        && flag == "--icon"
    {
        let n = xpaste_platform::app_icon(id).map_or(0, |b| {
            std::fs::write(std::env::temp_dir().join("xpaste-icon.png"), &b).ok();
            b.len()
        });
        println!("icon_bytes={n}");
        return;
    }
    let Some(s) = xpaste_platform::read_snapshot(true).expect("读取剪切板失败") else {
        println!("剪切板为空");
        return;
    };
    for f in &s.flavors {
        println!("item={} format={} bytes={}", f.item, f.format, f.data.len());
    }
    let yn = |b: bool| if b { "yes" } else { "no" };
    let bitmap = s
        .bitmap
        .as_ref()
        .map_or("no".into(), |b| format!("yes png_bytes={}", b.len()));
    println!(
        "text={} html={} files={} bitmap={bitmap} url_title={} sensitive={} source={}",
        yn(s.text.is_some()),
        yn(s.html.is_some()),
        s.file_paths.len(),
        yn(s.url_title.is_some()),
        s.sensitive,
        s.source.as_ref().map_or("-", |a| a.name.as_str()),
    );
}
