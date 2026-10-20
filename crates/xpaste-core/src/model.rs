use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::CoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Text,
    Link,
    Code,
    Image,
    Video,
    File,
}

const KINDS: [(Kind, &str); 6] = [
    (Kind::Text, "text"),
    (Kind::Link, "link"),
    (Kind::Code, "code"),
    (Kind::Image, "image"),
    (Kind::Video, "video"),
    (Kind::File, "file"),
];

impl Kind {
    /// 与 serde 一致的小写名，用于入库。
    pub fn as_str(&self) -> &'static str {
        KINDS.iter().find(|(k, _)| k == self).map_or("", |(_, s)| s)
    }

    pub fn parse(s: &str) -> Option<Kind> {
        KINDS.iter().find(|(_, n)| *n == s).map(|(k, _)| *k)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Tauri 快捷键格式
    pub hotkey: String,
    /// 7 | 30 | 90，None 表示永久
    pub retention_days: Option<u32>,
    /// 50..=5000，步长 50
    pub max_records: u32,
    pub ignored_apps: Vec<String>,
    pub skip_sensitive: bool,
    pub move_to_top: bool,
    pub launch_at_login: bool,
    pub appearance: Appearance,
    /// 逻辑像素
    pub panel_size: (f64, f64),
    pub last_filter: String,
    /// 复制频谱收起（只剩标题行）
    pub spectrum_collapsed: bool,
    /// 使用次数达到它即进入“常用”，2..=99
    pub frequent_min_uses: u32,
}

impl Default for Settings {
    fn default() -> Self {
        #[cfg(target_os = "macos")]
        let ignored_apps = vec!["1Password".into(), "钥匙串访问".into()];
        #[cfg(windows)]
        let ignored_apps = vec!["1Password".into(), "KeePass".into(), "Bitwarden".into()];
        #[cfg(not(any(target_os = "macos", windows)))]
        let ignored_apps = Vec::new();

        Self {
            hotkey: "CommandOrControl+Shift+V".into(),
            retention_days: Some(30),
            max_records: 500,
            ignored_apps,
            skip_sensitive: true,
            move_to_top: true,
            launch_at_login: true,
            appearance: Appearance::Dark,
            panel_size: (960.0, 640.0),
            last_filter: "all".into(),
            spectrum_collapsed: false,
            frequent_min_uses: 3,
        }
    }
}

const MODIFIERS: [&str; 12] = [
    "commandorcontrol",
    "cmdorctrl",
    "cmdorcontrol",
    "commandorctrl",
    "cmd",
    "command",
    "control",
    "ctrl",
    "alt",
    "option",
    "super",
    "meta",
];

impl Settings {
    /// 按顺序检查，返回第一条不满足的规则。
    pub fn validate(&self) -> Result<(), CoreError> {
        let fail = |m: &str| Err(CoreError::InvalidSettings(m.into()));

        let has_modifier = self
            .hotkey
            .split('+')
            .any(|part| MODIFIERS.contains(&part.trim().to_lowercase().as_str()));
        if !has_modifier {
            return fail("至少需要包含 Mod 或 Alt");
        }
        if !matches!(self.retention_days, None | Some(7 | 30 | 90)) {
            return fail("历史保留只能是 7 天、30 天、90 天或永久");
        }
        if !(50..=5000).contains(&self.max_records) || !self.max_records.is_multiple_of(50) {
            return fail("请输入 50 到 5000 之间、50 的整数倍");
        }
        if !(2..=99).contains(&self.frequent_min_uses) {
            return fail("请输入 2 到 99 之间的整数");
        }
        let (w, h) = self.panel_size;
        if !(w >= 720.0 && h >= 520.0) {
            return fail("面板尺寸不能小于 720 × 520");
        }
        Ok(())
    }
}

/// 剪切板里一个 item 的一种格式的原始数据。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Flavor {
    pub item: u32,
    pub format: String,
    pub data: Vec<u8>,
}

/// 不打印原始字节，只打印长度。
impl fmt::Debug for Flavor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Flavor")
            .field("item", &self.item)
            .field("format", &self.format)
            .field("data", &self.data.len())
            .finish()
    }
}

/// 来源应用：macOS 为 bundle id，Windows 为可执行文件完整路径。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppInfo {
    pub id: String,
    pub name: String,
}

/// 平台层读到的一次剪切板内容。`sensitive` 为 true 时不含任何内容。
/// `Debug` 只输出长度与标记，不输出内容、路径、标题。
#[derive(Default, Clone)]
pub struct Snapshot {
    pub flavors: Vec<Flavor>,
    pub text: Option<String>,
    pub html: Option<String>,
    pub file_paths: Vec<PathBuf>,
    pub bitmap: Option<Vec<u8>>,
    pub url_title: Option<String>,
    pub sensitive: bool,
    pub source: Option<AppInfo>,
}

impl fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Snapshot")
            .field("flavors", &self.flavors.len())
            .field("text_bytes", &self.text_bytes())
            .field("html", &self.html.is_some())
            .field("file_paths", &self.file_paths.len())
            .field("bitmap", &self.bitmap.as_ref().map(Vec::len))
            .field("url_title", &self.url_title.is_some())
            .field("sensitive", &self.sensitive)
            .field("source", &self.source.as_ref().map(|a| &a.name))
            .finish()
    }
}

impl Snapshot {
    /// 文本的 UTF-8 字节数，供日志使用（不暴露内容）。
    pub fn text_bytes(&self) -> usize {
        self.text.as_ref().map_or(0, String::len)
    }

    /// 有可展示内容：文本非空、有文件引用或有位图（设计 5.2 第 5 步）。
    pub fn has_presentable_content(&self) -> bool {
        self.text.as_deref().is_some_and(|t| !t.trim().is_empty())
            || !self.file_paths.is_empty()
            || self.bitmap.is_some()
    }
}

/// 待入库的一条记录（设计 5.3）。`Debug` 同 `Snapshot` 脱敏，不输出标题、正文、路径。
pub struct NewRecord {
    pub hash: [u8; 32],
    pub kind: Kind,
    pub title: String,
    pub text: Option<String>,
    pub paths: Vec<PathBuf>,
    pub link_title: Option<String>,
    pub lang: Option<&'static str>,
    pub size: u64,
    pub meta: Option<serde_json::Value>,
    pub search_text: String,
    pub app: Option<AppInfo>,
    pub flavors: Vec<Flavor>,
    pub bitmap: Option<Vec<u8>>,
}

impl fmt::Debug for NewRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewRecord")
            .field("kind", &self.kind)
            .field("size", &self.size)
            .field("flavors", &self.flavors.len())
            .field("paths", &self.paths.len())
            .field("text", &self.text.is_some())
            .field("bitmap", &self.bitmap.is_some())
            .field("lang", &self.lang)
            .field("app", &self.app.as_ref().map(|a| &a.name))
            .finish()
    }
}

/// 列表项。`thumb`、`app.icon` 为绝对文件路径，应用层再转 asset URL。
/// `Debug` 不输出标题与来源应用。
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordSummary {
    pub id: i64,
    pub kind: Kind,
    pub title: String,
    pub app: Option<AppSummary>,
    pub copied_at: i64,
    pub uses: u32,
    pub pinned: bool,
    pub size: u64,
    pub lang: Option<String>,
    pub thumb: Option<String>,
    pub duration_sec: Option<f64>,
}

impl fmt::Debug for RecordSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordSummary")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("size", &self.size)
            .field("uses", &self.uses)
            .field("pinned", &self.pinned)
            .field("copied_at", &self.copied_at)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSummary {
    pub name: String,
    pub icon: Option<String>,
}

/// 详情。`text` 超过 200 KB 时截断并置 `truncated`。`Debug` 同 `RecordSummary` 脱敏。
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordDetail {
    #[serde(flatten)]
    pub summary: RecordSummary,
    pub text: Option<String>,
    pub truncated: bool,
    pub link_title: Option<String>,
    pub files: Vec<FileEntry>,
    pub image: Option<ImageInfo>,
    pub video: Option<VideoInfo>,
}

impl fmt::Debug for RecordDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.summary.fmt(f)
    }
}

#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub ext: String,
    pub size: u64,
    pub exists: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInfo {
    pub src: String,
    pub w: u32,
    pub h: u32,
}

/// `records.meta` 的视频部分：与图片 `w`/`h` 同层的扁平键 `{duration,w,h,fps,codec}`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct VideoMeta {
    #[serde(rename = "duration")]
    pub duration_sec: f64,
    pub w: u32,
    pub h: u32,
    pub fps: f64,
    pub codec: String,
}

/// 视频详情。`src` 为源文件绝对路径（文件不存在时为 None），应用层再转 asset URL。
/// 时长、封面、字节数在摘要里，这里不重复。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoInfo {
    pub src: Option<String>,
    pub w: u32,
    pub h: u32,
    pub fps: f64,
    pub codec: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_text_bytes_counts_utf8() {
        let s = Snapshot {
            text: Some("你好".into()),
            ..Default::default()
        };
        assert_eq!(s.text_bytes(), 6);
        assert_eq!(Snapshot::default().text_bytes(), 0);
    }

    #[test]
    fn flavor_debug_hides_bytes() {
        let f = Flavor {
            item: 0,
            format: "public.utf8-plain-text".into(),
            data: b"secret-bytes".to_vec(),
        };
        let out = format!("{f:?}");
        // 派生的 Debug 会打印成字节数组 `[115, 101, …]`
        assert!(!out.contains('[') && out.contains("data: 12"), "{out}");
    }

    #[test]
    fn snapshot_debug_hides_content() {
        let s = Snapshot {
            text: Some("secret-xyz".into()),
            file_paths: vec!["/tmp/secret.txt".into()],
            source: Some(AppInfo {
                id: "C:\\Users\\secret-user\\app.exe".into(),
                name: "app".into(),
            }),
            ..Default::default()
        };
        let out = format!("{s:?}");
        assert!(
            !out.contains("secret-xyz") && !out.contains("secret.txt"),
            "{out}"
        );
        assert!(!out.contains("secret-user") && out.contains("app"), "{out}");
    }

    fn invalid(s: &Settings) -> String {
        match s.validate() {
            Err(CoreError::InvalidSettings(m)) => m,
            other => panic!("expected InvalidSettings, got {other:?}"),
        }
    }

    #[test]
    fn default_settings_match_spec() {
        let s = Settings::default();
        assert_eq!(s.hotkey, "CommandOrControl+Shift+V");
        assert_eq!(s.retention_days, Some(30));
        assert_eq!(s.max_records, 500);
        assert!(s.skip_sensitive && s.move_to_top && s.launch_at_login);
        assert_eq!(s.appearance, Appearance::Dark);
        assert_eq!(s.panel_size, (960.0, 640.0));
        assert_eq!(s.last_filter, "all");
        assert!(!s.spectrum_collapsed);
        assert_eq!(s.frequent_min_uses, 3);
        #[cfg(target_os = "macos")]
        assert_eq!(s.ignored_apps, vec!["1Password", "钥匙串访问"]);
        #[cfg(windows)]
        assert_eq!(s.ignored_apps, vec!["1Password", "KeePass", "Bitwarden"]);
        assert!(s.validate().is_ok());
    }

    #[test]
    fn missing_fields_take_defaults() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.max_records, 500);
        assert_eq!(s.frequent_min_uses, 3);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let s: Settings = serde_json::from_str(r#"{"maxRecords":100,"futureField":true}"#).unwrap();
        assert_eq!(s.max_records, 100);
    }

    #[test]
    fn serializes_camel_case() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(
            json.contains(r#""maxRecords":500"#)
                && json.contains(r#""retentionDays":30"#)
                && json.contains(r#""appearance":"dark""#)
                && json.contains(r#""frequentMinUses":3"#)
        );
    }

    #[test]
    fn rejects_max_records_33() {
        let s = Settings {
            max_records: 33,
            ..Default::default()
        };
        assert_eq!(invalid(&s), "请输入 50 到 5000 之间、50 的整数倍");
    }

    #[test]
    fn max_records_boundaries() {
        for ok in [50, 5000] {
            assert!(
                Settings {
                    max_records: ok,
                    ..Default::default()
                }
                .validate()
                .is_ok()
            );
        }
        for bad in [0, 5050] {
            assert!(
                Settings {
                    max_records: bad,
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
    }

    #[test]
    fn frequent_min_uses_boundaries() {
        let with = |n| Settings {
            frequent_min_uses: n,
            ..Default::default()
        };
        assert!(with(2).validate().is_ok() && with(99).validate().is_ok());
        assert_eq!(invalid(&with(1)), "请输入 2 到 99 之间的整数");
        assert_eq!(invalid(&with(100)), "请输入 2 到 99 之间的整数");
    }

    #[test]
    fn rejects_hotkey_without_modifier() {
        let s = Settings {
            hotkey: "C".into(),
            ..Default::default()
        };
        assert_eq!(invalid(&s), "至少需要包含 Mod 或 Alt");
    }

    #[test]
    fn hotkey_modifier_check_is_case_insensitive() {
        for hk in [
            "alt+c",
            "CommandOrControl+Alt+C",
            "Control+Shift+V",
            "Command+Shift+V",
            "CmdOrCtrl+Shift+V",
            "cmd+shift+v",
        ] {
            assert!(
                Settings {
                    hotkey: hk.into(),
                    ..Default::default()
                }
                .validate()
                .is_ok(),
                "{hk}"
            );
        }
        assert!(
            Settings {
                hotkey: "Shift+V".into(),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn rejects_unknown_retention() {
        let s = Settings {
            retention_days: Some(10),
            ..Default::default()
        };
        assert_eq!(invalid(&s), "历史保留只能是 7 天、30 天、90 天或永久");
    }

    #[test]
    fn rejects_panel_size_below_minimum() {
        let s = Settings {
            panel_size: (640.0, 400.0),
            ..Default::default()
        };
        assert_eq!(invalid(&s), "面板尺寸不能小于 720 × 520");
    }

    #[test]
    fn kind_str_roundtrips_like_serde() {
        for (k, name) in KINDS {
            assert_eq!(k.as_str(), name);
            assert_eq!(Kind::parse(name), Some(k));
            assert_eq!(serde_json::to_string(&k).unwrap(), format!("\"{name}\""));
        }
        assert_eq!(Kind::parse("nope"), None);
    }

    #[test]
    fn parse_rejects_color() {
        assert_eq!(Kind::parse("color"), None);
        assert_eq!(KINDS.len(), 6);
    }

    #[test]
    fn video_meta_uses_flat_meta_keys() {
        let m = VideoMeta {
            duration_sec: 3.0,
            w: 640,
            h: 360,
            fps: 30.0,
            codec: "H.264".into(),
        };
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            serde_json::json!({"duration": 3.0, "w": 640, "h": 360, "fps": 30.0, "codec": "H.264"})
        );
    }

    #[test]
    fn record_debug_hides_content() {
        let summary = RecordSummary {
            id: 1,
            kind: Kind::Text,
            title: "secret-title".into(),
            app: Some(AppSummary {
                name: "secret-app".into(),
                icon: None,
            }),
            copied_at: 0,
            uses: 0,
            pinned: false,
            size: 3,
            lang: None,
            thumb: None,
            duration_sec: None,
        };
        let d = RecordDetail {
            summary,
            text: Some("secret-text".into()),
            truncated: false,
            link_title: None,
            files: vec![],
            image: None,
            video: None,
        };
        let out = format!("{d:?} {:?}", d.summary);
        assert!(!out.contains("secret"), "{out}");
        let json = serde_json::to_string(&d).unwrap();
        assert!(
            json.contains(r#""copiedAt":0"#) && json.contains(r#""linkTitle":null"#),
            "{json}"
        );
    }
}
