//! 拼音与首字母搜索的核心：生成给 FTS 用的拼音索引文本，并在原文里定位被拼音命中的汉字。

use std::collections::HashSet;
use std::sync::OnceLock;

use pinyin::ToPinyinMulti;

/// 参与拼音索引的原文字符数上限。
pub const PY_MAX_CHARS: usize = 4096;

/// 查询是否按拼音处理：至少两个字符，且全是 ASCII 字母。
pub fn is_pinyin_query(q: &str) -> bool {
    q.len() >= 2 && q.bytes().all(|b| b.is_ascii_alphabetic())
}

/// 应用名是否对得上输入（来源筛选的建议，M29 规则 1）：名字的一部分（不分大小写，可省略空格）、
/// 全拼的开头或首字母。拉丁字母至少 2 个，汉字 1 个起。
pub fn name_matches(name: &str, q: &str) -> bool {
    let q = q.trim().to_lowercase();
    if q.is_empty() || (q.is_ascii() && q.len() < 2) {
        return false;
    }
    let n = name.to_lowercase();
    n.contains(&q)
        || n.replace(' ', "").contains(&q)
        || (is_pinyin_query(&q) && find(name, &q).is_some())
}

/// 一个汉字：`reads[0]` 是默认读音，其后为去重后的其他读音。
type Han = (char, Vec<&'static str>);

/// `c` 的可用读音，首个为默认读音；不是汉字或没有可用读音时为空。
fn reads_of(c: char) -> Vec<&'static str> {
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
    reads
}

/// 把 `chars` 切成连续汉字段。
fn han_runs(chars: impl Iterator<Item = char>) -> Vec<Vec<Han>> {
    let mut runs: Vec<Vec<Han>> = vec![];
    let mut cur: Vec<Han> = vec![];
    for c in chars {
        let reads = reads_of(c);
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

/// 查询切分用的音节与其非空前缀（含音节本身），从 `pinyin` 数据推出，每个进程只算一次。
/// 不含元音的叹词音节（n、m、ng、hm、hng）不参与切分，否则 'ha'ng 之类的冗余切法会挤占切分上限；索引侧照常写入。
fn syllables() -> &'static (HashSet<&'static str>, HashSet<&'static str>) {
    static SETS: OnceLock<(HashSet<&'static str>, HashSet<&'static str>)> = OnceLock::new();
    SETS.get_or_init(|| {
        let mut syl = HashSet::new();
        for c in ('\u{3400}'..='\u{9fff}').chain('\u{20000}'..='\u{2a6df}') {
            syl.extend(reads_of(c));
        }
        syl.retain(|s| s.contains(['a', 'e', 'i', 'o', 'u', 'v']));
        let pre = syl
            .iter()
            .flat_map(|s| (1..=s.len()).map(move |i| &s[..i]))
            .collect();
        (syl, pre)
    })
}

/// 全拼行：每个音节前加 `'`，如 `'hui'yi`，让查询只能从音节开头命中。
fn full_line(rs: &[&str]) -> String {
    rs.iter().map(|r| format!("'{r}")).collect()
}

/// 首字母行：大写，如 `HY`，在区分大小写的 trigram 里与全拼行互不串扰。
fn initials_line(rs: &[&str]) -> String {
    rs.iter()
        .map(|r| r.as_bytes()[0].to_ascii_uppercase() as char)
        .collect()
}

/// `s` 前 `PY_MAX_CHARS` 个字符的拼音索引（全拼行、首字母行、多音字窗口行，按行），无汉字时为空。
pub fn index_text(s: &str) -> String {
    let mut seen = HashSet::new();
    let mut lines: Vec<String> = vec![];
    let mut push = |l: String| {
        if seen.insert(l.clone()) {
            lines.push(l);
        }
    };
    for run in han_runs(s.chars().take(PY_MAX_CHARS)) {
        let def: Vec<&str> = run.iter().map(|h| h.1[0]).collect();
        push(full_line(&def));
        push(initials_line(&def));
        // 多音字：只在 i 前后的窗口里换读音，避免长段落的行数爆炸；全拼窗口 ±4，够覆盖四字词（如 'zhao'shang'yin'hang）；
        // 首字母一个字才一字节，窗口放宽到 ±8，让整句首字母（如 CQHG）也能带上多音字读音；
        // 其他读音的首字母与默认读音相同时（的 de / di），首字母窗口行与默认行重复，不输出
        for (i, (_, reads)) in run.iter().enumerate() {
            for alt in &reads[1..] {
                let win = |radius: usize| {
                    let (lo, hi) = (i.saturating_sub(radius), (i + radius + 1).min(run.len()));
                    let mut w = def[lo..hi].to_vec();
                    w[i - lo] = alt;
                    w
                };
                push(full_line(&win(4)));
                if alt.as_bytes()[0] != def[i].as_bytes()[0] {
                    push(initials_line(&win(8)));
                }
            }
        }
    }
    lines.join("\n")
}

// ponytail: 音节切分最多取 32 种，长查询多出的切法直接丢弃（先试长音节，自然切法排在前面）；漏召回时再按段数排序或放宽上限
const MAX_SPLITS: usize = 32;

/// 拼音查询的最长字母数（M05 规则 11）：切分按音节递归，深度随长度增长，粘贴超长字母串会撑爆线程栈
const MAX_QUERY: usize = 64;

/// `q` 的全部查询短语：每种音节切分一个 `'s1's2…'sk`（末段可为音节前缀），最后是大写的首字母短语。
/// `q` 不是 3–64 个 ASCII 字母时为空。
fn phrases(q: &str) -> Vec<String> {
    if !(3..=MAX_QUERY).contains(&q.len()) || !q.bytes().all(|b| b.is_ascii_alphabetic()) {
        return vec![];
    }
    let q = q.to_ascii_lowercase();
    let (syl, pre) = syllables();
    // ok[i]：q[i..] 能切完。先倒着算好，深度优先只走切得完的分支，长查询（如粘贴的长英文串）不会在死路上指数级空转
    let mut ok = vec![false; q.len()];
    for i in (0..q.len()).rev() {
        // 前缀集里最长 6 个字母，剩余更长就不必对整段后缀求哈希（粘贴超长查询时避免 O(L²)）
        ok[i] = (q.len() - i <= 6 && pre.contains(&q[i..]))
            || (1..(q.len() - i).min(7)).any(|n| syl.contains(&q[i..i + n]) && ok[i + n]);
    }
    let mut out = vec![];
    split(&q, 0, &ok, &mut String::new(), &mut out);
    out.push(q.to_ascii_uppercase());
    out
}

/// 深度优先枚举 `q[i..]` 的音节切分，`acc` 是已切出的短语前缀。
fn split(q: &str, i: usize, ok: &[bool], acc: &mut String, out: &mut Vec<String>) {
    let (syl, pre) = syllables();
    let rest = &q[i..];
    if out.len() < MAX_SPLITS && rest.len() <= 6 && pre.contains(rest) {
        out.push(format!("{acc}'{rest}"));
    }
    // 中间段必须是完整音节（最长 6 个字母），且后面还要剩下能切完的字母
    for n in (1..rest.len().min(7)).rev() {
        if out.len() >= MAX_SPLITS {
            return;
        }
        if ok[i + n] && syl.contains(&rest[..n]) {
            let len = acc.len();
            acc.push('\'');
            acc.push_str(&rest[..n]);
            split(q, i + n, ok, acc, out);
            acc.truncate(len);
        }
    }
}

/// `q`（3–64 个 ASCII 字母）对 `fts_py` 的 MATCH 表达式：各短语加双引号以 ` OR ` 连接；否则为 None。
pub fn match_expr(q: &str) -> Option<String> {
    let ps = phrases(q);
    // 短语只含 ASCII 字母与 `'`，不会出现需要转义的双引号
    debug_assert!(
        ps.iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_alphabetic() || b == b'\''))
    );
    (!ps.is_empty()).then(|| {
        ps.iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(" OR ")
    })
}

/// `index`（`index_text` 的输出）是否包含 `match_expr(q)` 的任一短语，与区分大小写的 trigram MATCH 结果一致。
pub fn index_matches(index: &str, q: &str) -> bool {
    phrases(q).iter().any(|p| index.contains(p.as_str()))
}

/// 从 `run[0]` 的读音开头起以全拼匹配 `q`，返回覆盖的字数；中间各字的读音要用完，末字可在音节中间止。
fn full_len(run: &[Han], q: &str) -> Option<usize> {
    let (_, reads) = run.first()?;
    for r in reads {
        if r.starts_with(q) {
            return Some(1);
        }
        if let Some(rest) = q.strip_prefix(r)
            && let Some(n) = full_len(&run[1..], rest)
        {
            return Some(n + 1);
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
            let n = full.then(|| full_len(tail, q)).flatten();
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
    fn name_matches_by_substring_pinyin_and_initials() {
        assert!(name_matches("微信", "wx"));
        assert!(name_matches("微信", "weixin"));
        assert!(name_matches("微信", "wei"));
        assert!(name_matches("企业微信", "微信"));
        assert!(name_matches("微信", "微"));
        assert!(name_matches("VS Code", "vs"));
        assert!(name_matches("VS Code", "vscode"));
        assert!(name_matches("Chrome", " CHR "));
        // 单个拉丁字母不算：打第一个字母就跳出建议太吵
        assert!(!name_matches("Chrome", "c"));
        assert!(!name_matches("微信", ""));
        assert!(!name_matches("微信", "发票"));
        assert!(!name_matches("Chrome", "wx"));
    }

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
        assert_eq!(index_text("xPaste 发布"), "'fa'bu\nFB");
        assert_eq!(index_text("hello 123"), "");
        let t = index_text("周四会议纪要");
        assert!(t.contains("'zhou'si'hui'yi'ji'yao"), "{t}");
        assert!(t.contains("ZSHYJY"), "{t}");
    }

    #[test]
    fn index_text_covers_common_polyphones() {
        for (s, full, ini) in [
            ("招商银行", "'yin'hang", "ZSYH"),
            ("重庆火锅", "'chong'qing", "CQHG"),
            ("音乐", "'yin'yue", "YY"),
        ] {
            let t = index_text(s);
            assert!(t.contains(full) && t.contains(ini), "{s}: {t}");
        }
    }

    #[test]
    fn index_text_caps_source_chars() {
        let t = index_text(&"会".repeat(PY_MAX_CHARS + 1000));
        assert_eq!(t.lines().next().unwrap(), "'hui".repeat(PY_MAX_CHARS));
    }

    #[test]
    fn non_ascii_readings_do_not_panic() {
        // 欸 / 誒 的读音含 ê（非 ASCII），按输入法写成 e，不能让按字节切片越界
        let t = index_text("欸，会议");
        assert!(t.contains("'hui'yi") && t.contains("HY"), "{t}");
        assert_eq!(find("欸会议", "huiyi").as_deref(), Some("会议"));
        assert_eq!(find("欸会议", "hy").as_deref(), Some("会议"));
        let _ = index_text("誒欸");
        // ê 不再丢掉，而是作为读音 e 保留
        assert!(han_runs("欸".chars())[0][0].1.contains(&"e"));
    }

    #[test]
    fn u_umlaut_is_typed_as_v() {
        let t = index_text("旅游");
        assert!(t.contains("'lv'you") && t.contains("LY"), "{t}");
        assert!(index_text("律师").contains("'lv'shi"));
        assert!(
            index_text("女人").starts_with("'nv'ren"),
            "{}",
            index_text("女人")
        );
        assert_eq!(find("去旅游吧", "lvyou").as_deref(), Some("旅游"));
        assert_eq!(find("考虑一下", "kaolv").as_deref(), Some("考虑"));
    }

    #[test]
    fn full_pinyin_window_spans_four_chars() {
        for (s, full) in [
            ("招商银行", "'zhao'shang'yin'hang"),
            ("重庆火锅", "'chong'qing'huo'guo"),
            ("中国银行", "'zhong'guo'yin'hang"),
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
        assert_eq!(find("招商银行卡", "yinhang").as_deref(), Some("银行"));
        assert_eq!(find("重庆火锅", "cq").as_deref(), Some("重庆"));
        assert_eq!(find("abc 会议", "abc"), None);
        assert_eq!(find("会议", "xyz"), None);
    }

    #[test]
    fn index_lines_mark_syllables_and_upper_initials() {
        assert_eq!(index_text("会议"), "'hui'yi\nHY\n'kuai'yi\nKY"); // 会 kuai 首字母不同，保留窗口首字母行
        let t = index_text("的"); // de / di 首字母相同：不输出重复的首字母窗口行
        assert!(
            t.contains("'de") && t.contains("'di") && t.matches('D').count() == 1,
            "{t}"
        );
    }

    #[test]
    fn match_expr_segments_syllables() {
        assert_eq!(match_expr("hy"), None);
        let e = match_expr("xian").unwrap();
        assert!(
            e.contains("\"'xian\"") && e.contains("\"'xi'an\"") && e.contains("\"XIAN\""),
            "{e}"
        );
        assert_eq!(match_expr("ing").unwrap(), "\"ING\""); // 没有音节以 ing 开头
    }

    #[test]
    fn segmentation_skips_vowelless_syllables() {
        let e = match_expr("zhaoshangyinhang").unwrap();
        assert!(e.contains("\"'zhao'shang'yin'hang\""), "{e}");
        assert!(e.split(" OR ").count() < 32, "{e}");
        let e = match_expr("ngai").unwrap();
        assert!(!e.split(" OR ").any(|p| p.starts_with("\"'ng'")), "{e}");
    }

    #[test]
    fn long_unsplittable_query_is_fast() {
        // an 可切成 an / a'n…，结尾 ii 又切不完：不能在死路上指数级枚举
        let t = std::time::Instant::now();
        let q = format!("{}ii", "an".repeat(20));
        assert_eq!(match_expr(&q).unwrap(), format!("\"{}\"", q.to_uppercase()));
        assert!(t.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn very_long_splittable_query_does_not_overflow_stack() {
        // 粘贴进搜索框的长串 a 每个字母都是音节：切分递归深度等于长度，tokio 工作线程栈只有 2 MiB
        let q = "a".repeat(200_000);
        let r = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || match_expr(&q))
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(r, None);
    }

    #[test]
    fn no_mid_syllable_matches() {
        for (text, q, hit) in [
            ("上海", "hang", false),
            ("北京", "ing", false),
            ("上海", "ang", false),
            ("安定", "and", true),
            ("会议", "huiy", true),
            ("会议", "uiyi", false),
            ("西安", "xian", true),
            ("先生", "xian", true),
            ("会议纪要", "hyjy", true),
        ] {
            assert_eq!(index_matches(&index_text(text), q), hit, "{text} / {q}");
        }
        assert_eq!(find("会议纪要", "uiyij"), None);
        assert_eq!(find("会议纪要", "huiy").as_deref(), Some("会议"));
    }
}
