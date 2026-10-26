//! 拼音与首字母搜索的核心：生成给 FTS 用的拼音索引文本，并在原文里定位被拼音命中的汉字。

use std::collections::HashSet;

use pinyin::ToPinyinMulti;

/// 参与拼音索引的原文字符数上限。
pub const PY_MAX_CHARS: usize = 4096;

/// 查询是否按拼音处理：至少两个字符，且全是 ASCII 字母。
pub fn is_pinyin_query(q: &str) -> bool {
    q.len() >= 2 && q.bytes().all(|b| b.is_ascii_alphabetic())
}

/// 一个汉字：`reads[0]` 是默认读音，其后为去重后的其他读音。
type Han = (char, Vec<&'static str>);

/// 把 `chars` 切成连续汉字段。
fn han_runs(chars: impl Iterator<Item = char>) -> Vec<Vec<Han>> {
    let mut runs: Vec<Vec<Han>> = vec![];
    let mut cur: Vec<Han> = vec![];
    for c in chars {
        // 读音带声调变体会重复，按出现顺序去重（首个即默认读音）；
        // ü 与 ê 先换成输入法的写法（ü 打 v），其余非纯 ASCII 小写的读音会让按字节切片越界，直接丢掉
        let mut reads: Vec<&'static str> = vec![];
        for r in c.to_pinyin_multi().into_iter().flatten() {
            let p = match r.plain() {
                "nü" => "nv",
                "lü" => "lv",
                "nüe" => "nve",
                "lüe" => "lve",
                "ê" => "e",
                p => p,
            };
            let ascii = !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase());
            if ascii && !reads.contains(&p) {
                reads.push(p);
            }
        }
        // 没有可用读音的字当作非汉字，截断汉字段
        if reads.is_empty() {
            if !cur.is_empty() {
                runs.push(std::mem::take(&mut cur));
            }
            continue;
        }
        cur.push((c, reads));
    }
    if !cur.is_empty() {
        runs.push(cur);
    }
    runs
}

/// `s` 前 `PY_MAX_CHARS` 个字符的小写拼音索引（全拼、首字母、多音字变体，按行），无汉字时为空。
pub fn index_text(s: &str) -> String {
    let mut seen = HashSet::new();
    let mut lines: Vec<String> = vec![];
    let mut push = |l: String| {
        if seen.insert(l.clone()) {
            lines.push(l);
        }
    };
    for run in han_runs(s.chars().take(PY_MAX_CHARS)) {
        fn join(rs: &[&str]) -> String {
            rs.concat()
        }
        fn ini(rs: &[&str]) -> String {
            rs.iter().map(|r| &r[..1]).collect()
        }
        let def: Vec<&str> = run.iter().map(|h| h.1[0]).collect();
        push(join(&def));
        push(ini(&def));
        // 多音字：只在 i 前后的窗口里换读音，避免长段落的行数爆炸；全拼窗口 ±4，够覆盖四字词（如 zhaoshangyinhang）；
        // 首字母一个字才一字节，窗口放宽到 ±8，让整句首字母（如 cqhg）也能带上多音字读音
        for (i, (_, reads)) in run.iter().enumerate() {
            for alt in &reads[1..] {
                for (radius, f) in [(4, join as fn(&[&str]) -> String), (8, ini)] {
                    let (lo, hi) = (i.saturating_sub(radius), (i + radius + 1).min(run.len()));
                    let mut w = def[lo..hi].to_vec();
                    w[i - lo] = alt;
                    push(f(&w));
                }
            }
        }
    }
    lines.join("\n")
}

/// 从 `run[0]` 起以全拼匹配 `q`，返回覆盖的字数；首字可从音节中间起，末字可在音节中间止。
fn full_len(run: &[Han], q: &str, first: bool) -> Option<usize> {
    let (_, reads) = run.first()?;
    for r in reads {
        let starts = if first { 0..r.len() } else { 0..1 };
        for st in starts {
            let p = &r[st..];
            if p.starts_with(q) {
                return Some(1);
            }
            if let Some(rest) = q.strip_prefix(p)
                && let Some(n) = full_len(&run[1..], rest, false)
            {
                return Some(n + 1);
            }
        }
    }
    None
}

/// 从 `run[0]` 起以首字母匹配 `q`，返回覆盖的字数。
fn initials_len(run: &[Han], q: &str) -> Option<usize> {
    let n = q.len();
    let ok = run.len() >= n
        && run
            .iter()
            .zip(q.bytes())
            .all(|((_, reads), b)| reads.iter().any(|r| r.as_bytes()[0] == b));
    ok.then_some(n)
}

/// `s` 中被（小写的）`q` 以全拼或首字母命中的最靠前一段汉字，返回原字。只看前 `PY_MAX_CHARS` 个字符，与索引一致。
pub fn find(s: &str, q: &str) -> Option<String> {
    find_by(s, q, true)
}

/// 同 `find`，但只按首字母匹配。
pub fn find_initials(s: &str, q: &str) -> Option<String> {
    find_by(s, q, false)
}

fn find_by(s: &str, q: &str, full: bool) -> Option<String> {
    if q.is_empty() {
        return None;
    }
    for run in han_runs(s.chars().take(PY_MAX_CHARS)) {
        for i in 0..run.len() {
            let tail = &run[i..];
            let n = full.then(|| full_len(tail, q, true)).flatten();
            if let Some(n) = n.or_else(|| initials_len(tail, q)) {
                return Some(tail[..n].iter().map(|h| h.0).collect());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_pinyin_query_rules() {
        assert!(is_pinyin_query("hy"));
        assert!(is_pinyin_query("huiyi"));
        assert!(!is_pinyin_query("h"));
        assert!(!is_pinyin_query("hui yi"));
        assert!(!is_pinyin_query("会yi"));
        assert!(!is_pinyin_query("js2"));
        assert!(!is_pinyin_query(""));
    }

    #[test]
    fn index_text_has_full_and_initials_per_run() {
        assert_eq!(index_text("xPaste 发布"), "fabu\nfb");
        assert_eq!(index_text("hello 123"), "");
        let t = index_text("周四会议纪要");
        assert!(t.contains("zhousihuiyijiyao"), "{t}");
        assert!(t.contains("zshyjy"), "{t}");
    }

    #[test]
    fn index_text_covers_common_polyphones() {
        for (s, full, ini) in [
            ("招商银行", "yinhang", "zsyh"),
            ("重庆火锅", "chongqing", "cqhg"),
            ("音乐", "yinyue", "yy"),
        ] {
            let t = index_text(s);
            assert!(t.contains(full) && t.contains(ini), "{s}: {t}");
        }
    }

    #[test]
    fn index_text_caps_source_chars() {
        let t = index_text(&"会".repeat(PY_MAX_CHARS + 1000));
        assert_eq!(t.lines().next().unwrap(), "hui".repeat(PY_MAX_CHARS));
    }

    #[test]
    fn non_ascii_readings_do_not_panic() {
        // 欸 / 誒 的读音含 ê（非 ASCII），按输入法写成 e，不能让按字节切片越界
        let t = index_text("欸，会议");
        assert!(t.contains("huiyi") && t.contains("hy"), "{t}");
        assert_eq!(find("欸会议", "huiyi").as_deref(), Some("会议"));
        assert_eq!(find("欸会议", "hy").as_deref(), Some("会议"));
        let _ = index_text("誒欸");
        // ê 不再丢掉，而是作为读音 e 保留
        assert!(han_runs("欸".chars())[0][0].1.contains(&"e"));
    }

    #[test]
    fn u_umlaut_is_typed_as_v() {
        let t = index_text("旅游");
        assert!(t.contains("lvyou") && t.contains("ly"), "{t}");
        assert!(index_text("律师").contains("lvshi"));
        assert!(
            index_text("女人").starts_with("nvren"),
            "{}",
            index_text("女人")
        );
        assert_eq!(find("去旅游吧", "lvyou").as_deref(), Some("旅游"));
        assert_eq!(find("考虑一下", "kaolv").as_deref(), Some("考虑"));
    }

    #[test]
    fn full_pinyin_window_spans_four_chars() {
        for (s, full) in [
            ("招商银行", "zhaoshangyinhang"),
            ("重庆火锅", "chongqinghuoguo"),
            ("中国银行", "zhongguoyinhang"),
        ] {
            let t = index_text(s);
            assert!(t.contains(full), "{s}: {t}");
        }
    }

    #[test]
    fn find_initials_ignores_full_pinyin() {
        assert_eq!(find_initials("周四会议纪要", "hy").as_deref(), Some("会议"));
        assert_eq!(find_initials("重庆火锅", "cq").as_deref(), Some("重庆"));
        assert_eq!(find_initials("会议", "hu"), None);
        assert_eq!(find_initials("会议", ""), None);
    }

    #[test]
    fn find_scans_only_indexed_prefix() {
        let s = format!("{}会议", "，".repeat(PY_MAX_CHARS));
        assert_eq!(find(&s, "huiyi"), None);
        assert_eq!(find_initials(&s, "hy"), None);
        let s = format!("{}会议", "，".repeat(PY_MAX_CHARS - 2));
        assert_eq!(find(&s, "huiyi").as_deref(), Some("会议"));
    }

    #[test]
    fn find_returns_matched_han() {
        assert_eq!(find("周四会议纪要", "huiyi").as_deref(), Some("会议"));
        assert_eq!(find("会议纪要", "hyjy").as_deref(), Some("会议纪要"));
        assert_eq!(find("会议", "huiy").as_deref(), Some("会议"));
        assert_eq!(find("会议纪要", "uiyij").as_deref(), Some("会议纪"));
        assert_eq!(find("招商银行卡", "yinhang").as_deref(), Some("银行"));
        assert_eq!(find("重庆火锅", "cq").as_deref(), Some("重庆"));
        assert_eq!(find("abc 会议", "abc"), None);
        assert_eq!(find("会议", "xyz"), None);
    }
}
