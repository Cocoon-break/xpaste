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
        Kind::Link => url_title.or_else(|| anchor_text(html.as_deref()?, text.as_deref()?.trim())),
        _ => None,
    };
    let lang = match kind {
        Kind::Code => text.as_deref().and_then(detect::lang),
        _ => None,
    };

    let title = match kind {
        Kind::Text | Kind::Code => text
            .as_deref()
            .and_then(|t| t.lines().map(str::trim).find(|l| !l.is_empty()))
            .map(|l| l.chars().take(TITLE_MAX_CHARS).collect()),
        Kind::Link => link_title.clone().or_else(|| non_blank(text.as_deref())),
        Kind::Color => non_blank(text.as_deref()).map(|t| t.to_uppercase()),
        Kind::Image | Kind::Video | Kind::File => match paths.as_slice() {
            [] => bitmap
                .as_ref()
                .map(|_| format!("截图 {}.png", local_stamp(now_ms))),
            [p] => Some(file_name(p)),
            [p, ..] => Some(format!("{} 等 {} 个", file_name(p), paths.len())),
        },
    }
    .unwrap_or_else(|| EMPTY_TITLE.into());

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
    } else if is_ignored(&snapshot, &settings.ignored_apps) {
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

/// 按应用名称匹配，去首尾空白、不区分大小写（含非 ASCII）。
#[cfg(feature = "store")]
fn is_ignored(snapshot: &Snapshot, ignored: &[String]) -> bool {
    let Some(app) = &snapshot.source else {
        return false;
    };
    let name = app.name.trim().to_lowercase();
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

/// `YYYY-MM-DD HH.mm`。
// ponytail: UTC；本地时间由前端按 copiedAt 显示
fn local_stamp(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant 的 civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}.{:02}",
        sod / 3600,
        sod % 3600 / 60
    )
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
    fn color_title_is_upper_hex() {
        assert_eq!(
            build_record(snap(" #6f8bff "), Kind::Color, 0).title,
            "#6F8BFF"
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
    fn local_stamp_is_utc_civil() {
        assert_eq!(local_stamp(0), "1970-01-01 00.00");
        assert_eq!(local_stamp(951_782_400_000), "2000-02-29 00.00");
        assert_eq!(local_stamp(1_792_348_701_015), "2026-10-18 18.38");
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
            assert!(matches!(
                run(s, &Settings::default(), false),
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
