//! 采集管线：过滤快照并入库（设计 5.2），由快照计算入库字段（设计 5.3）。

use std::path::Path;

use crate::{Kind, NewRecord, Snapshot, detect};

#[cfg(feature = "store")]
use crate::store::{Store, Upsert};
#[cfg(feature = "store")]
use crate::{CoreError, Settings};

pub const MAX_TEXT_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_BITMAP_BYTES: usize = 50 * 1024 * 1024;
pub const MAX_FLAVOR_BYTES: usize = 50 * 1024 * 1024;

const TITLE_MAX_CHARS: usize = 200;
const EMPTY_TITLE: &str = "(空)";
/// 文件记录里文本类文件读入搜索文本的总预算（每条记录）
pub const FILE_TEXT_BYTES: usize = 64 * 1024;
/// 按扩展名判定的文本类文件：内容进搜索文本（只搜，不显示）。无扩展名、`.env` 一类点文件不在其中
const TEXT_EXTS: &[&str] = &[
    "txt", "md", "markdown", "csv", "tsv", "json", "yaml", "yml", "toml", "xml", "html", "htm",
    "css", "js", "mjs", "ts", "jsx", "tsx", "py", "rs", "go", "java", "kt", "swift", "c", "h",
    "cpp", "hpp", "cc", "m", "rb", "php", "sh", "zsh", "bash", "sql", "log", "ini", "conf", "cfg",
    "srt", "tex", "rtf",
];

/// 由快照计算一条待入库记录。`bitmap` 与 `flavors` 从快照移入，不复制。
pub fn build_record(snapshot: Snapshot, kind: Kind, now_ms: i64) -> NewRecord {
    let hash = detect::content_hash(&snapshot, kind);
    let Snapshot {
        flavors,
        text,
        html,
        file_paths: paths,
        bitmap,
        url_title,
        source: app,
        ..
    } = snapshot;
    let is_file_kind = matches!(kind, Kind::Image | Kind::Video | Kind::File);

    let link_title = match kind {
        Kind::Link => url_title
            .filter(|t| !t.trim().is_empty())
            .or_else(|| anchor_text(html.as_deref()?, text.as_deref()?.trim())),
        _ => None,
    };
    let lang = match kind {
        Kind::Code => text.as_deref().and_then(detect::lang),
        _ => None,
    };

    let title: String = match kind {
        Kind::Text | Kind::Code => text
            .as_deref()
            .and_then(|t| t.lines().map(str::trim).find(|l| !l.is_empty()))
            .map(Into::into),
        Kind::Link => link_title.clone().or_else(|| non_blank(text.as_deref())),
        Kind::Image | Kind::Video | Kind::File => match paths.as_slice() {
            [] => bitmap
                .as_ref()
                .map(|_| format!("截图 {}.png", local_stamp(now_ms))),
            [p] => Some(file_name(p)),
            [p, ..] => Some(format!("{} 等 {} 个", file_name(p), paths.len())),
        },
    }
    .map_or_else(
        || EMPTY_TITLE.into(),
        |t| t.chars().take(TITLE_MAX_CHARS).collect(),
    );

    let size = if !is_file_kind {
        text.as_ref().map_or(0, |t| t.len() as u64)
    } else if !paths.is_empty() {
        paths.iter().map(|p| file_len(p)).sum()
    } else {
        bitmap.as_ref().map_or(0, |b| b.len() as u64)
    };

    let mut parts: Vec<String> = Vec::new();
    parts.extend(text.clone());
    parts.extend(link_title.clone());
    parts.extend(paths.iter().map(|p| file_name(p)));
    parts.extend(paths.iter().map(|p| p.display().to_string()));
    parts.extend(app.as_ref().map(|a| a.name.clone()));
    // 标题也要能搜到：位图截图没有文本与路径，否则只剩来源应用名可搜
    parts.push(title.clone());
    if kind == Kind::File {
        parts.extend(files_text(&paths, FILE_TEXT_BYTES));
    }
    let search_text = parts.join("\n").to_lowercase();

    NewRecord {
        hash,
        kind,
        title,
        text,
        paths,
        link_title,
        lang,
        size,
        meta: None,
        search_text,
        app,
        flavors,
        bitmap,
    }
}

#[cfg(feature = "store")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    Paused,
    Sensitive,
    IgnoredApp,
    TooLarge,
    Empty,
}

/// 第二个元素是被容量清理删除的 id。
#[cfg(feature = "store")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Skipped(Skip),
    Added(i64, Vec<i64>),
    Merged(i64, Vec<i64>),
}

/// 设计 5.2 的 1–7 步：依次过滤，通过后识别类型、去重入库。
#[cfg(feature = "store")]
pub fn process(
    mut snapshot: Snapshot,
    settings: &Settings,
    paused: bool,
    store: &mut Store,
    now_ms: i64,
) -> Result<Outcome, CoreError> {
    let skip = if paused {
        Some(Skip::Paused)
    } else if snapshot.sensitive && settings.skip_sensitive {
        Some(Skip::Sensitive)
    } else if snapshot
        .source
        .as_ref()
        .is_some_and(|a| is_ignored_app(&a.name, &settings.ignored_apps))
    {
        Some(Skip::IgnoredApp)
    } else if snapshot.text_bytes() > MAX_TEXT_BYTES
        || snapshot
            .bitmap
            .as_ref()
            .is_some_and(|b| b.len() > MAX_BITMAP_BYTES)
    {
        Some(Skip::TooLarge)
    } else if !snapshot.has_presentable_content() {
        Some(Skip::Empty)
    } else {
        None
    };
    if let Some(s) = skip {
        return Ok(Outcome::Skipped(s));
    }

    let kind = detect::kind(&snapshot);
    snapshot.flavors.retain(|f| {
        let keep = f.data.len() <= MAX_FLAVOR_BYTES;
        if !keep {
            log::debug!("丢弃过大的 flavor：{} {} 字节", f.format, f.data.len());
        }
        keep
    });
    let record = build_record(snapshot, kind, now_ms);
    Ok(
        match store.insert_or_merge(&record, now_ms, settings.max_records)? {
            (Upsert::Added(id), removed) => Outcome::Added(id, removed),
            (Upsert::Merged(id), removed) => Outcome::Merged(id, removed),
        },
    )
}

/// 按应用名称匹配，去首尾空白、不区分大小写（含非 ASCII）。监听回调读剪切板前也用它。
pub fn is_ignored_app(name: &str, ignored: &[String]) -> bool {
    let name = name.trim().to_lowercase();
    !name.is_empty() && ignored.iter().any(|i| i.trim().to_lowercase() == name)
}

fn non_blank(t: Option<&str>) -> Option<String> {
    t.map(str::trim).filter(|t| !t.is_empty()).map(Into::into)
}

fn file_name(p: &Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// 目录与不存在的文件记 0。
/// 文件记录中文本类文件的内容，按顺序读到预算用完；非 UTF-8（二进制）与读不到的文件跳过。
fn files_text(paths: &[std::path::PathBuf], budget: usize) -> Vec<String> {
    let mut left = budget;
    let mut out = Vec::new();
    for p in paths {
        if left == 0 {
            break;
        }
        let is_text = p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| TEXT_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)));
        if !is_text {
            continue;
        }
        let Some(t) = read_text_prefix(p, left) else {
            continue;
        };
        left -= t.len();
        out.push(t);
    }
    out
}

/// 读文件前 `max` 字节；末尾被截断的多字节字符丢掉，中间出现无效 UTF-8 或 NUL 视为二进制返回 None。
fn read_text_prefix(p: &Path, max: usize) -> Option<String> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(p)
        .ok()?
        .take(max as u64)
        .read_to_end(&mut buf)
        .ok()?;
    if buf.contains(&0) {
        return None;
    }
    let valid = match std::str::from_utf8(&buf) {
        Ok(_) => buf.len(),
        // 只有读满 max 时才可能截在字符中间；其他位置的错误是二进制
        Err(e) if e.error_len().is_none() && buf.len() == max => e.valid_up_to(),
        Err(_) => return None,
    };
    buf.truncate(valid);
    let t = String::from_utf8(buf).ok()?;
    (!t.trim().is_empty()).then_some(t)
}

fn file_len(p: &Path) -> u64 {
    std::fs::metadata(p)
        .map(|m| if m.is_file() { m.len() } else { 0 })
        .unwrap_or(0)
}

/// HTML 恰为单个 `<a href="url">标题</a>` 时返回标题文字。
fn anchor_text(html: &str, url: &str) -> Option<String> {
    let html = html.trim();
    let inner = html.strip_prefix("<a ")?.strip_suffix("</a>")?;
    let (attrs, label) = inner.split_once('>')?;
    let label = label.trim();
    let ok =
        attrs.contains(&format!("href=\"{url}\"")) && !label.is_empty() && !label.contains('<');
    ok.then(|| label.into())
}

/// 本地时间 `YYYY-MM-DD HH.mm`；夏令时边界取"现在"的偏移。
fn local_stamp(ms: i64) -> String {
    stamp_at(ms, *chrono::Local::now().offset())
}

fn stamp_at(ms: i64, offset: chrono::FixedOffset) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .with_timezone(&offset)
        .format("%Y-%m-%d %H.%M")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppInfo;

    fn snap(t: &str) -> Snapshot {
        Snapshot {
            text: Some(t.into()),
            ..Default::default()
        }
    }
    fn snap_files(p: &[&str]) -> Snapshot {
        Snapshot {
            file_paths: p.iter().map(Into::into).collect(),
            ..Default::default()
        }
    }
    fn snap_bitmap() -> Snapshot {
        Snapshot {
            bitmap: Some(vec![0x89, b'P', b'N', b'G', 1, 2, 3]),
            ..Default::default()
        }
    }

    #[test]
    fn text_title_is_first_nonblank_line_truncated() {
        let r = build_record(snap("\n\n  标题行  \n第二行"), Kind::Text, 0);
        assert_eq!(r.title, "标题行");
        let long = "字".repeat(300);
        assert_eq!(
            build_record(snap(&long), Kind::Text, 0)
                .title
                .chars()
                .count(),
            200
        );
    }
    #[test]
    fn link_title_prefers_url_title_then_anchor_html_then_url() {
        let mut s = snap("https://a.io/x");
        s.url_title = Some("A 站".into());
        assert_eq!(build_record(s, Kind::Link, 0).title, "A 站");
        let mut s = snap("https://a.io/x");
        s.html = Some(r#"<a href="https://a.io/x">锚文本</a>"#.into());
        let r = build_record(s, Kind::Link, 0);
        assert_eq!(r.link_title.as_deref(), Some("锚文本"));
        assert_eq!(
            build_record(snap("https://a.io/x"), Kind::Link, 0).title,
            "https://a.io/x"
        );
    }
    #[test]
    fn blank_url_title_falls_back_to_url() {
        let mut s = snap("https://a.io/x");
        s.url_title = Some("  \n".into());
        let r = build_record(s, Kind::Link, 0);
        assert_eq!(r.title, "https://a.io/x");
        assert_eq!(r.link_title, None);
    }
    #[test]
    fn link_title_is_capped_at_200_chars() {
        let mut s = snap("https://a.io/x");
        s.url_title = Some("页".repeat(300));
        assert_eq!(build_record(s, Kind::Link, 0).title.chars().count(), 200);
        let long = format!("https://a.io/{}", "x".repeat(300));
        assert_eq!(
            build_record(snap(&long), Kind::Link, 0)
                .title
                .chars()
                .count(),
            200
        );
    }
    #[test]
    fn ignored_app_name_matching() {
        let list = vec![" 1Password ".to_string(), "Ärger".into()];
        assert!(is_ignored_app("1PASSWORD", &list));
        assert!(is_ignored_app("ärger ", &list));
        assert!(!is_ignored_app("Notes", &list));
        assert!(!is_ignored_app("  ", &["".into()]));
    }
    #[test]
    fn file_title_counts_multiple() {
        assert_eq!(
            build_record(snap_files(&["/d/规范.fig"]), Kind::File, 0).title,
            "规范.fig"
        );
        assert_eq!(
            build_record(snap_files(&["/d/a.png", "/d/b.png"]), Kind::Image, 0).title,
            "a.png 等 2 个"
        );
    }
    #[test]
    fn bitmap_title_is_timestamped_png() {
        let t = build_record(snap_bitmap(), Kind::Image, 1_792_348_701_015).title;
        assert!(t.starts_with("截图 ") && t.ends_with(".png"), "{t}");
    }
    #[test]
    fn size_rules() {
        assert_eq!(build_record(snap("你好"), Kind::Text, 0).size, 6);
        assert_eq!(
            build_record(snap_bitmap(), Kind::Image, 0).size,
            snap_bitmap().bitmap.unwrap().len() as u64
        );
        assert_eq!(
            build_record(snap_files(&["/definitely/missing"]), Kind::File, 0).size,
            0
        );
    }
    #[test]
    fn search_text_is_lowercased_union() {
        let mut s = snap_files(&["/Docs/Spec.PDF"]);
        s.source = Some(AppInfo {
            id: "f".into(),
            name: "Finder".into(),
        });
        let st = build_record(s, Kind::File, 0).search_text;
        assert!(st.contains("spec.pdf") && st.contains("/docs/spec.pdf") && st.contains("finder"));
    }
    #[test]
    fn text_files_content_is_searchable_within_budget() {
        let dir = std::env::temp_dir().join(format!("xpaste-capture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let notes = dir.join("会议纪要.md");
        std::fs::write(&notes, "# 季度预算\nBudget 2026 讨论").unwrap();
        let bin = dir.join("data.json");
        std::fs::write(&bin, [b'{', 0xff, 0xfe, 0, b'}']).unwrap();
        let big = dir.join("big.log");
        std::fs::write(&big, "字".repeat(FILE_TEXT_BYTES)).unwrap();
        let other = dir.join("deck.pptx");
        std::fs::write(&other, "pptx-text-inside").unwrap();

        let st = build_record(
            snap_files(&[
                notes.to_str().unwrap(),
                bin.to_str().unwrap(),
                other.to_str().unwrap(),
            ]),
            Kind::File,
            0,
        )
        .search_text;
        assert!(
            st.contains("季度预算") && st.contains("budget 2026"),
            "{st}"
        );
        assert!(!st.contains("pptx-text-inside"), "非文本扩展名不读内容");
        assert!(!st.contains('\u{fffd}'), "二进制文件应整个跳过");

        // 预算用完即停：big.log 占满后 notes 不再读入；截断不留半个字符
        let st = build_record(
            snap_files(&[big.to_str().unwrap(), notes.to_str().unwrap()]),
            Kind::File,
            0,
        )
        .search_text;
        assert!(!st.contains("季度预算"), "{}", st.len());
        assert!(st.len() < FILE_TEXT_BYTES + 1024);

        // 图片类型的记录不读文件内容
        let st = build_record(snap_files(&[notes.to_str().unwrap()]), Kind::Image, 0).search_text;
        assert!(!st.contains("季度预算"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn bitmap_title_is_searchable() {
        let r = build_record(snap_bitmap(), Kind::Image, 1_792_348_701_015);
        assert!(
            r.title.starts_with("截图 ") && r.title.ends_with(".png"),
            "{}",
            r.title
        );
        assert!(r.search_text.contains(&r.title), "{}", r.search_text);
    }
    #[test]
    fn stamp_at_applies_offset() {
        use chrono::FixedOffset;
        assert_eq!(
            stamp_at(1_792_348_701_015, FixedOffset::east_opt(8 * 3600).unwrap()),
            "2026-10-19 02.38"
        );
        assert_eq!(
            stamp_at(0, FixedOffset::west_opt(5 * 3600).unwrap()),
            "1969-12-31 19.00"
        );
    }
    #[test]
    fn debug_hides_content() {
        let mut s = snap_files(&["/tmp/secret.txt"]);
        s.text = Some("secret-xyz".into());
        let out = format!("{:?}", build_record(s, Kind::File, 0));
        assert!(!out.contains("secret"), "{out}");
    }
    #[test]
    fn code_gets_lang() {
        assert_eq!(
            build_record(snap("git status"), Kind::Code, 0).lang,
            Some("Shell")
        );
    }

    #[cfg(feature = "store")]
    mod process {
        use super::snap;
        use crate::capture::*;
        use crate::store::tests::tmp;
        use crate::{AppInfo, Flavor, Settings, Store};

        fn mem() -> Store {
            Store::open_in_memory(&tmp()).unwrap()
        }
        fn run(s: Snapshot, cfg: &Settings, paused: bool) -> Outcome {
            process(s, cfg, paused, &mut mem(), 1).unwrap()
        }

        #[test]
        fn skips_when_paused() {
            assert!(matches!(
                run(snap("a"), &Settings::default(), true),
                Outcome::Skipped(Skip::Paused)
            ));
        }
        /// 开关关闭时平台层会读出内容（`read_snapshot(false)`），这里的快照即对应那种情况。
        #[test]
        fn skips_sensitive_when_enabled_records_when_disabled() {
            let mut s = Snapshot {
                sensitive: true,
                ..snap("pw")
            };
            assert!(matches!(
                run(s.clone(), &Settings::default(), false),
                Outcome::Skipped(Skip::Sensitive)
            ));
            let cfg = Settings {
                skip_sensitive: false,
                ..Default::default()
            };
            s.sensitive = true;
            assert!(matches!(run(s, &cfg, false), Outcome::Added(..)));
        }
        #[test]
        fn skips_ignored_app_case_insensitively() {
            let mut s = snap("x");
            s.source = Some(AppInfo {
                id: "p".into(),
                name: "1PASSWORD".into(),
            });
            let cfg = Settings {
                ignored_apps: vec!["1Password".into()],
                ..Default::default()
            };
            assert!(matches!(
                run(s, &cfg, false),
                Outcome::Skipped(Skip::IgnoredApp)
            ));
        }
        #[test]
        fn ignored_app_match_trims_and_handles_non_ascii() {
            let cfg = Settings {
                ignored_apps: vec![" KeePass ".into(), "Ärger".into()],
                ..Default::default()
            };
            for name in ["keepass", "ärger "] {
                let mut s = snap("x");
                s.source = Some(AppInfo {
                    id: "p".into(),
                    name: name.into(),
                });
                assert!(
                    matches!(run(s, &cfg, false), Outcome::Skipped(Skip::IgnoredApp)),
                    "{name}"
                );
            }
        }
        #[test]
        fn skips_oversized_text_and_bitmap() {
            assert!(matches!(
                run(
                    snap(&"x".repeat(MAX_TEXT_BYTES + 1)),
                    &Settings::default(),
                    false
                ),
                Outcome::Skipped(Skip::TooLarge)
            ));
            let s = Snapshot {
                bitmap: Some(vec![0; MAX_BITMAP_BYTES + 1]),
                ..Default::default()
            };
            assert!(matches!(
                run(s, &Settings::default(), false),
                Outcome::Skipped(Skip::TooLarge)
            ));
        }
        #[test]
        fn skips_snapshot_without_presentable_content() {
            let mut s = Snapshot::default();
            s.flavors.push(Flavor {
                item: 0,
                format: "com.private".into(),
                data: vec![1],
            });
            assert!(matches!(
                run(s, &Settings::default(), false),
                Outcome::Skipped(Skip::Empty)
            ));
        }
        #[test]
        fn adds_then_merges() {
            let mut st = mem();
            let cfg = Settings::default();
            let a = process(snap("a"), &cfg, false, &mut st, 1).unwrap();
            let b = process(snap("a"), &cfg, false, &mut st, 2).unwrap();
            assert!(matches!(a, Outcome::Added(..)));
            assert!(matches!(b, Outcome::Merged(..)));
        }
        #[test]
        fn oversized_flavor_is_dropped_not_record() {
            let mut s = snap("a");
            s.flavors.push(Flavor {
                item: 0,
                format: "huge".into(),
                data: vec![0; MAX_FLAVOR_BYTES + 1],
            });
            let mut st = mem();
            let Outcome::Added(id, _) =
                process(s, &Settings::default(), false, &mut st, 1).unwrap()
            else {
                panic!()
            };
            assert!(st.flavors(id).unwrap().iter().all(|f| f.format != "huge"));
        }
    }
}
