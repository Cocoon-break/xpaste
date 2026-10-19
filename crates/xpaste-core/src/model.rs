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
    Color,
    Image,
    Video,
    File,
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
            appearance: Appearance::System,
            panel_size: (960.0, 640.0),
            last_filter: "all".into(),
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
        let (w, h) = self.panel_size;
        if !(w >= 720.0 && h >= 520.0) {
            return fail("面板尺寸不能小于 720 × 520");
        }
        Ok(())
    }
}

/// 剪切板里一个 item 的一种格式的原始数据。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Flavor {
    pub item: u32,
    pub format: String,
    pub data: Vec<u8>,
}

/// 来源应用：macOS 为 bundle id，Windows 为可执行文件完整路径。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppInfo {
    pub id: String,
    pub name: String,
}

/// 平台层读到的一次剪切板内容。`sensitive` 为 true 时不含任何内容。
/// `Debug` 只输出长度与标记，不输出内容、路径、标题。
#[derive(Default)]
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
        assert_eq!(s.appearance, Appearance::System);
        assert_eq!(s.panel_size, (960.0, 640.0));
        assert_eq!(s.last_filter, "all");
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
                && json.contains(r#""appearance":"system""#)
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
}
