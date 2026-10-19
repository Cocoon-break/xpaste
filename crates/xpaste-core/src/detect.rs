//! M02 类型识别、代码语言识别与去重哈希。规则都是手写匹配，不引入正则库。

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::{Kind, Snapshot};

pub const VIDEO_EXTS: [&str; 8] = ["mp4", "mov", "m4v", "mkv", "avi", "webm", "wmv", "flv"];
pub const IMAGE_EXTS: [&str; 10] = [
    "png", "jpg", "jpeg", "gif", "webp", "heic", "bmp", "tif", "tiff", "svg",
];

/// 代码编辑器与终端（设计 5.4）。`*` 结尾为前缀匹配；`.exe` 按文件名、不区分大小写；其余精确匹配。
pub const CODE_APPS: &[&str] = &[
    "com.microsoft.VSCode",
    "com.todesktop.230313mzl4w4u92", // Cursor
    "com.jetbrains.*",
    "com.apple.dt.Xcode",
    "com.sublimetext.*",
    "com.apple.Terminal",
    "com.googlecode.iterm2",
    "Code.exe",
    "Cursor.exe",
    "sublime_text.exe",
    "WindowsTerminal.exe",
    "powershell.exe",
    "pwsh.exe",
];

/// JetBrains Windows 可执行文件名前缀，形如 `idea64.exe`。
const JETBRAINS_EXES: [&str; 10] = [
    "idea", "pycharm", "webstorm", "goland", "clion", "rider", "phpstorm", "rubymine", "datagrip",
    "fleet",
];

/// 区分大小写，与 M02 规则 7 原文一致（否则 “Let me know…” 这类句子会被判成代码）。
const CODE_PREFIXES: [&str; 9] = [
    "$ ",
    "git ",
    "npm ",
    "pnpm ",
    "SELECT ",
    "const ",
    "let ",
    "function ",
    "import ",
];

/// `id` 为 macOS bundle ID，或 Windows 可执行文件完整路径（按文件名比较）。
pub fn is_code_app(id: &str) -> bool {
    let exe = id.rsplit(['\\', '/']).next().unwrap_or(id).to_lowercase();
    let listed = CODE_APPS.iter().any(|app| {
        if let Some(prefix) = app.strip_suffix('*') {
            id.starts_with(prefix)
        } else if app.ends_with(".exe") {
            exe == app.to_lowercase()
        } else {
            id == *app
        }
    });
    listed
        || exe
            .strip_suffix("64.exe")
            .is_some_and(|stem| JETBRAINS_EXES.iter().any(|j| stem.starts_with(j)))
}

/// 严格按 M02 顺序 1–8 判定。
pub fn kind(snapshot: &Snapshot) -> Kind {
    let files = &snapshot.file_paths;
    if !files.is_empty() {
        let all = |exts: &[&str]| files.iter().all(|p| has_ext(p, exts));
        return if all(&VIDEO_EXTS) {
            Kind::Video
        } else if all(&IMAGE_EXTS) {
            Kind::Image
        } else {
            Kind::File
        };
    }
    let text = snapshot.text.as_deref().unwrap_or("").trim();
    // 规则 4：有非空白文本时位图只作预览（表格、文档会附带渲染图），按文本规则继续
    if snapshot.bitmap.is_some() && text.is_empty() {
        return Kind::Image;
    }
    if is_color(text) {
        Kind::Color
    } else if is_link(text) {
        Kind::Link
    } else if is_code(text, snapshot) {
        Kind::Code
    } else {
        Kind::Text
    }
}

/// 设计 5.3 的语言识别，按顺序：Shell → SQL → JavaScript。
pub fn lang(text: &str) -> Option<&'static str> {
    let t = text.trim();
    if ["$ ", "git ", "npm ", "pnpm "]
        .iter()
        .any(|p| t.starts_with(p))
    {
        Some("Shell")
    } else if ["SELECT", "INSERT", "UPDATE", "WITH"]
        .iter()
        .any(|p| starts_with_word_ci(t, p))
    {
        Some("SQL")
    } else if ["const ", "let ", "function ", "=>", "import "]
        .iter()
        .any(|p| t.contains(p))
    {
        Some("JavaScript")
    } else {
        None
    }
}

/// 去重用的 SHA-256（设计 5.4）。前缀区分文件引用、位图与文本。
pub fn content_hash(snapshot: &Snapshot, kind: Kind) -> [u8; 32] {
    let mut h = Sha256::new();
    let is_file_kind = matches!(kind, Kind::Image | Kind::Video | Kind::File);
    if is_file_kind && !snapshot.file_paths.is_empty() {
        let mut paths: Vec<_> = snapshot
            .file_paths
            .iter()
            .map(|p| p.to_string_lossy())
            .collect();
        paths.sort();
        h.update(b"files\n");
        h.update(paths.join("\n"));
    } else if is_file_kind && let Some(png) = &snapshot.bitmap {
        h.update(b"bitmap\n");
        h.update(png);
    } else {
        h.update(b"text\n");
        h.update(snapshot.text.as_deref().unwrap_or(""));
    }
    h.finalize().into()
}

fn has_ext(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// 不区分大小写的前缀，且其后为空白或文本结束（单词边界）。
fn starts_with_word_ci(text: &str, word: &str) -> bool {
    text.get(..word.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(word))
        && text[word.len()..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace)
}

/// `^#([0-9a-f]{3}|[0-9a-f]{6})$`，不区分大小写。
fn is_color(t: &str) -> bool {
    t.strip_prefix('#')
        .is_some_and(|hex| matches!(hex.len(), 3 | 6) && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// `^https?://\S+$`
fn is_link(t: &str) -> bool {
    t.strip_prefix("https://")
        .or_else(|| t.strip_prefix("http://"))
        .is_some_and(|rest| !rest.is_empty() && !rest.chars().any(char::is_whitespace))
}

fn is_code(t: &str, snapshot: &Snapshot) -> bool {
    snapshot.source.as_ref().is_some_and(|a| is_code_app(&a.id))
        || (t.contains('\n') && t.contains(['{', '}', ';', '=', '(', ')', '<', '>']))
        || CODE_PREFIXES.iter().any(|p| t.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppInfo, Kind, Snapshot};

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
    fn ac_m02_01_hex_colors() {
        for c in ["#6F8BFF", "#fff", "  #ABC  "] {
            assert_eq!(kind(&snap(c)), Kind::Color, "{c}");
        }
    }
    #[test]
    fn ac_m02_02_plain_url_is_link() {
        assert_eq!(kind(&snap("https://github.com/")), Kind::Link);
    }
    #[test]
    fn ac_m02_03_url_inside_text_is_text() {
        assert_eq!(kind(&snap("见 https://github.com/")), Kind::Text);
    }
    #[test]
    fn ac_m02_04_code_editor_source_is_code() {
        let mut s = snap("let x = 1");
        s.source = Some(AppInfo {
            id: "com.microsoft.VSCode".into(),
            name: "Code".into(),
        });
        assert_eq!(kind(&s), Kind::Code);
    }
    #[test]
    fn ac_m02_05_shell_prefix_is_code() {
        assert_eq!(kind(&snap("git log --oneline")), Kind::Code);
    }
    #[test]
    fn ac_m02_06_bitmap_is_image() {
        assert_eq!(kind(&snap_bitmap()), Kind::Image);
    }
    #[test]
    fn bitmap_with_text_is_text() {
        let s = Snapshot {
            text: Some("季度报表\t123".into()),
            ..snap_bitmap()
        };
        assert_eq!(kind(&s), Kind::Text);
        assert_eq!(
            content_hash(&s, Kind::Text),
            content_hash(&snap("季度报表\t123"), Kind::Text)
        );
    }
    #[test]
    fn bitmap_with_blank_text_is_image() {
        let s = Snapshot {
            text: Some("  \n".into()),
            ..snap_bitmap()
        };
        assert_eq!(kind(&s), Kind::Image);
    }
    #[test]
    fn ac_m02_07_mp4_is_video() {
        assert_eq!(kind(&snap_files(&["/v/a.MP4"])), Kind::Video);
    }
    #[test]
    fn ac_m02_08_png_is_image() {
        assert_eq!(kind(&snap_files(&["/p/a.png"])), Kind::Image);
    }
    #[test]
    fn ac_m02_09_fig_is_file() {
        assert_eq!(kind(&snap_files(&["/d/a.fig"])), Kind::File);
    }
    #[test]
    fn ac_m02_10_chinese_sentence_is_text() {
        assert_eq!(kind(&snap("明天上午十点开会。")), Kind::Text);
    }
    #[test]
    fn multiline_with_braces_is_code() {
        assert_eq!(kind(&snap("fn a() {\n  1\n}")), Kind::Code);
    }
    #[test]
    fn mixed_files_are_file() {
        assert_eq!(kind(&snap_files(&["/a.png", "/b.mp4"])), Kind::File);
        assert_eq!(kind(&snap_files(&["/a.png", "/b.JPG"])), Kind::Image);
    }
    #[test]
    fn jetbrains_prefix_matches() {
        let mut s = snap("x");
        s.source = Some(AppInfo {
            id: "com.jetbrains.intellij".into(),
            name: "IDEA".into(),
        });
        assert_eq!(kind(&s), Kind::Code);
    }
    #[test]
    fn lang_detection_order() {
        assert_eq!(lang("$ ls"), Some("Shell"));
        assert_eq!(lang("select * from t"), Some("SQL"));
        assert_eq!(lang("const a = () => 1"), Some("JavaScript"));
        assert_eq!(lang("hello"), None);
        assert_eq!(lang("withStyles(x)"), None);
        assert_eq!(lang("update users set a=1"), Some("SQL"));
    }
    #[test]
    fn hash_is_stable_and_order_independent_for_files() {
        assert_eq!(
            content_hash(&snap_files(&["/b", "/a"]), Kind::File),
            content_hash(&snap_files(&["/a", "/b"]), Kind::File)
        );
        assert_ne!(
            content_hash(&snap("a"), Kind::Text),
            content_hash(&snap("b"), Kind::Text)
        );
    }
    #[test]
    fn hash_prefix_separates_kinds() {
        // 同一个字符串作为文件路径与作为纯文本，不能算同一条
        assert_ne!(
            content_hash(&snap_files(&["/tmp/x"]), Kind::File),
            content_hash(&snap("/tmp/x"), Kind::Text)
        );
    }
    #[test]
    fn presentable_content() {
        assert!(!Snapshot::default().has_presentable_content());
        assert!(snap("a").has_presentable_content());
        assert!(!snap("  \n\t").has_presentable_content());
    }

    #[test]
    fn code_app_matches_windows_exe_paths() {
        assert!(is_code_app(r"C:\Program Files\Microsoft VS Code\CODE.EXE"));
        assert!(is_code_app(r"C:\JetBrains\bin\pycharm64.exe"));
        assert!(!is_code_app(r"C:\Windows\notepad.exe"));
        assert!(!is_code_app("com.microsoft.VSCodeInsidersX"));
    }

    #[test]
    fn prose_starting_with_capitalised_keyword_is_text() {
        for t in [
            "Let me know tomorrow",
            "Select all files first",
            "Import duties apply",
        ] {
            assert_eq!(kind(&snap(t)), Kind::Text, "{t}");
        }
        for t in ["SELECT 1", "let x = 1"] {
            assert_eq!(kind(&snap(t)), Kind::Code, "{t}");
        }
    }
}
