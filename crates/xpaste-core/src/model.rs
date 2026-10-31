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

/// 录屏清晰度（M24 规则 6）：高按屏幕实际像素，标准按逻辑尺寸，小文件在标准上再压低码率。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecQuality {
    High,
    #[default]
    Standard,
    Small,
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
    /// 识别图片中的文字（本机离线）
    pub ocr_enabled: bool,
    /// 为已有图片补识别
    pub ocr_backfill: bool,
    /// 复制文件时把文本类文件的内容读进搜索文本（每条最多 64 KB）；默认关：大库里的大段文本多半来自它
    pub search_file_text: bool,
    /// 截图全局快捷键；空字符串表示不启用
    pub shot_hotkey: String,
    /// 截图保存文件夹；空字符串表示平台默认（图片/xPaste）
    pub shot_dir: String,
    /// 保存截图时同时复制到剪切板
    pub shot_save_copy: bool,
    /// 录屏全局快捷键；空字符串表示不启用（M24）
    pub rec_hotkey: String,
    /// 录屏保存文件夹；空字符串表示平台默认（影片/xPaste）
    pub rec_dir: String,
    /// 录制系统声音
    pub rec_system_audio: bool,
    /// 录制麦克风
    pub rec_microphone: bool,
    /// 录完把视频文件复制到剪切板
    pub rec_copy: bool,
    /// 录屏清晰度（M24 规则 6）
    pub rec_quality: RecQuality,
    /// 录屏帧率，30 | 60（M24 规则 6）
    pub rec_fps: u32,
    /// 翻译引擎：`""`（还没选，由应用层按平台决定）| `"system"` | `"openai"`（M26 规则 6）
    pub translate_engine: String,
    /// OpenAI 兼容接口的 Base URL
    pub translate_base_url: String,
    pub translate_api_key: String,
    pub translate_model: String,
    /// 目标语言代码，见 `translate::Lang`
    pub translate_lang: String,
    /// 划词翻译全局快捷键；空字符串表示不启用（M15、M26 规则 18）
    pub translate_hotkey: String,
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
            ocr_enabled: true,
            ocr_backfill: true,
            search_file_text: false,
            shot_hotkey: "Alt+Shift+A".into(),
            shot_dir: String::new(),
            shot_save_copy: true,
            rec_hotkey: "Alt+Shift+R".into(),
            rec_dir: String::new(),
            rec_system_audio: false,
            rec_microphone: false,
            rec_copy: true,
            rec_quality: RecQuality::Standard,
            rec_fps: 30,
            translate_engine: String::new(),
            translate_base_url: "https://api.openai.com/v1".into(),
            translate_api_key: String::new(),
            translate_model: String::new(),
            translate_lang: "zh-Hans".into(),
            translate_hotkey: "Alt+Shift+T".into(),
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

fn has_modifier(s: &str) -> bool {
    s.split('+')
        .any(|part| MODIFIERS.contains(&part.trim().to_lowercase().as_str()))
}

/// 按 `+` 切分、trim、小写、修饰键别名归一、排序去重，用来比较两个快捷键是否是同一组合（M15）。
fn hotkey_key(s: &str) -> Vec<String> {
    hotkey_key_on(s, cfg!(target_os = "macos"))
}

/// 别名同全局快捷键插件的解析：`CommandOrControl` 在 macOS 上是 ⌘，在 Windows 上是 Ctrl。
fn hotkey_key_on(s: &str, mac: bool) -> Vec<String> {
    let mut parts: Vec<String> = s
        .split('+')
        .map(|p| {
            let p = p.trim().to_lowercase();
            match p.as_str() {
                "commandorcontrol" | "cmdorctrl" | "cmdorcontrol" | "commandorctrl" if mac => {
                    "super".into()
                }
                "commandorcontrol" | "cmdorctrl" | "cmdorcontrol" | "commandorctrl" => {
                    "control".into()
                }
                "cmd" | "command" | "super" | "meta" => "super".into(),
                "ctrl" | "control" => "control".into(),
                "option" | "alt" => "alt".into(),
                _ => p,
            }
        })
        .collect();
    parts.sort();
    parts.dedup();
    parts
}

impl Settings {
    /// 逐项读出已保存的设置（M15）：某一项读不出（类型不对、降级后遇到新版本加的取值）或使读出的设置
    /// 通不过校验时，只把这一项回落到默认值，其余保留；返回设置与回落了的字段名。整段不是 JSON 对象时报错。
    /// ponytail: 每项都整份反序列化加校验一次，字段数平方级，几十项无所谓
    pub fn from_json_lenient(raw: &str) -> Result<(Self, Vec<String>), serde_json::Error> {
        use serde_json::{Map, Value};
        let stored: Map<String, Value> = serde_json::from_str(raw)?;
        let Value::Object(mut merged) = serde_json::to_value(Self::default())? else {
            unreachable!("Settings 序列化为对象");
        };
        let usable = |m: &Map<String, Value>| {
            serde_json::from_value::<Self>(Value::Object(m.clone())).is_ok_and(|mut s| {
                s.disable_clashing_hotkeys();
                s.validate().is_ok()
            })
        };
        let mut dropped = Vec::new();
        for (k, v) in stored {
            // 新版本才有的字段：旧版不认识，照旧忽略
            let Some(old) = merged.insert(k.clone(), v) else {
                merged.remove(&k);
                continue;
            };
            if !usable(&merged) {
                merged.insert(k.clone(), old);
                dropped.push(k);
            }
        }
        let mut s: Self = serde_json::from_value(Value::Object(merged))?;
        s.disable_clashing_hotkeys();
        Ok((s, dropped))
    }

    /// 截图、录屏快捷键与别的快捷键是同一组合时停用它（M15、M24）：升级前把唤起设成了 `Alt+Shift+A`
    /// 或 `Alt+Shift+R` 的用户读出时会撞上新功能的默认值，不处理的话整份设置校验不过、回落到默认值。
    pub fn disable_clashing_hotkeys(&mut self) {
        if !self.shot_hotkey.is_empty() && hotkey_key(&self.shot_hotkey) == hotkey_key(&self.hotkey)
        {
            self.shot_hotkey.clear();
        }
        if !self.rec_hotkey.is_empty() {
            let rec = hotkey_key(&self.rec_hotkey);
            if rec == hotkey_key(&self.hotkey)
                || (!self.shot_hotkey.is_empty() && rec == hotkey_key(&self.shot_hotkey))
            {
                self.rec_hotkey.clear();
            }
        }
        // 翻译快捷键同理（M15）：升级前唤起、截图、录屏已用了 Alt+Shift+T 的，读出为不启用
        if self.translate_hotkey_clash().is_some() {
            self.translate_hotkey.clear();
        }
    }

    /// 翻译快捷键与哪个快捷键是同一组合（按唤起、截图、录屏的顺序）；录屏只在 macOS 上比（M24：别处不注册录屏快捷键）。
    fn translate_hotkey_clash(&self) -> Option<&'static str> {
        if self.translate_hotkey.is_empty() {
            return None;
        }
        let tr = hotkey_key(&self.translate_hotkey);
        [
            ("唤起", self.hotkey.as_str()),
            ("截图", self.shot_hotkey.as_str()),
            (
                "录屏",
                if cfg!(target_os = "macos") {
                    self.rec_hotkey.as_str()
                } else {
                    ""
                },
            ),
        ]
        .into_iter()
        .find(|(_, k)| !k.is_empty() && hotkey_key(k) == tr)
        .map(|(name, _)| name)
    }

    /// 翻译的三个文本字段去首尾空白，Base URL 再去掉末尾的 `/`（M26 规则 6）；保存设置时在 `validate` 之前调用。
    pub fn normalize(&mut self) {
        for f in [
            &mut self.translate_base_url,
            &mut self.translate_api_key,
            &mut self.translate_model,
        ] {
            *f = f.trim().to_owned();
        }
        self.translate_base_url = self.translate_base_url.trim_end_matches('/').to_owned();
    }

    /// 按顺序检查，返回第一条不满足的规则。
    pub fn validate(&self) -> Result<(), CoreError> {
        let fail = |m: &str| Err(CoreError::InvalidSettings(m.into()));

        if !has_modifier(&self.hotkey) {
            return fail("至少需要包含 Mod 或 Alt");
        }
        if !self.shot_hotkey.is_empty() {
            if !has_modifier(&self.shot_hotkey) {
                return fail("截图快捷键至少需要包含 Mod 或 Alt");
            }
            if hotkey_key(&self.shot_hotkey) == hotkey_key(&self.hotkey) {
                return fail("截图快捷键不能与唤起快捷键相同");
            }
        }
        // 录屏只在 macOS 上做（M24）：别处不注册录屏快捷键、设置里也没有这一行，不校验，免得占住用户看不到的组合
        if cfg!(target_os = "macos") && !self.rec_hotkey.is_empty() {
            if !has_modifier(&self.rec_hotkey) {
                return fail("录屏快捷键至少需要包含 Mod 或 Alt");
            }
            let rec = hotkey_key(&self.rec_hotkey);
            if rec == hotkey_key(&self.hotkey) {
                return fail("录屏快捷键不能与唤起快捷键相同");
            }
            if !self.shot_hotkey.is_empty() && rec == hotkey_key(&self.shot_hotkey) {
                return fail("录屏快捷键不能与截图快捷键相同");
            }
        }
        if !self.translate_hotkey.is_empty() && !has_modifier(&self.translate_hotkey) {
            return fail("翻译快捷键至少需要包含 Mod 或 Alt");
        }
        if let Some(name) = self.translate_hotkey_clash() {
            return fail(&format!("翻译快捷键不能与{name}快捷键相同"));
        }
        if !matches!(self.translate_engine.as_str(), "" | "system" | "openai") {
            return fail("翻译引擎只能是系统翻译或 OpenAI 兼容接口");
        }
        if crate::translate::Lang::from_code(&self.translate_lang).is_none() {
            return fail("不支持这种翻译目标语言");
        }
        // 选了系统翻译时设置里不显示接口三项，不拿它们拦保存（M26 规则 6）
        let url = &self.translate_base_url;
        if self.translate_engine != "system"
            && !url.is_empty()
            && !url.starts_with("http://")
            && !url.starts_with("https://")
        {
            return fail("请输入 http:// 或 https:// 开头的地址");
        }
        if !matches!(self.rec_fps, 30 | 60) {
            return fail("录屏帧率只能是 30 或 60");
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

/// `records.ocr_state` 的取值（设计 §3）。
pub mod ocr_state {
    pub const PENDING: u8 = 0;
    pub const TEXT: u8 = 1;
    pub const EMPTY: u8 = 2;
    pub const FAILED: u8 = 3;
    pub const SKIPPED: u8 = 4;
    pub const UNSUPPORTED: u8 = 5;
}

/// 搜索命中。`snippet` 只在命中来自识别文字（标题不含关键词）时给出；`mark` 是拼音命中时被匹配到的原汉字（字面命中为 None）。不派生 `Debug`：片段是内容。
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub id: i64,
    pub snippet: Option<String>,
    pub mark: Option<String>,
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
    /// 见 [`ocr_state`]；非图片记录恒为 0，不被消费
    pub ocr_state: u8,
    /// 不为 None 即片段，值是列表第二行的摘要（M25 规则 10）；前端按它判断，不看 pinned
    pub snippet_body: Option<String>,
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
    /// 识别出的原文（未小写）
    pub ocr_text: Option<String>,
    /// 最新一份译文及其目标语言代码（M26 规则 12）
    pub translation: Option<String>,
    pub translation_lang: Option<String>,
    /// 翻译方式标签：`system` 或 `openai:{模型 ID}`（M26 规则 16）
    pub translation_engine: Option<String>,
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

/// 纯文本 flavor。
pub fn text_flavor(text: &str) -> Flavor {
    if cfg!(windows) {
        // 存储口径：UTF-16LE、不带结尾 NUL（写回时 winfmt::write_bytes 补上）。
        // 输入框里的换行是 LF，CF_UNICODETEXT 约定 CRLF：记事本等老程序只认 CRLF
        Flavor {
            item: 0,
            format: "CF_UNICODETEXT".into(),
            data: crlf(text)
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect(),
        }
    } else {
        Flavor {
            item: 0,
            format: "public.utf8-plain-text".into(),
            data: text.as_bytes().to_vec(),
        }
    }
}

/// 换行统一成 CRLF：单独的 LF、CR 都换成 CRLF，已有的 CRLF 不会叠成两个。
fn crlf(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n")
}

/// 片段名字：去首尾空白，非空且不超过 100 个字符（M25 规则 10）。
pub fn snippet_name(raw: &str) -> Result<String, CoreError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(CoreError::InvalidSettings("请填写名字".into()));
    }
    if name.chars().count() > 100 {
        return Err(CoreError::InvalidSettings("名字不能超过 100 个字".into()));
    }
    Ok(name.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_normalizes_line_breaks_without_doubling() {
        for (a, b) in [
            ("a\nb", "a\r\nb"),
            ("a\r\nb", "a\r\nb"),
            ("a\rb", "a\r\nb"),
            ("a\n", "a\r\n"),
            ("a\r", "a\r\n"),
            ("\n\r\n\r", "\r\n\r\n\r\n"),
            ("无换行", "无换行"),
        ] {
            assert_eq!(crlf(a), b, "{a:?}");
        }
    }

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
    fn translate_settings_are_trimmed_and_validated() {
        let mut s = Settings {
            translate_api_key: " sk-1\n".into(),
            translate_model: " gpt ".into(),
            translate_base_url: " http://localhost:11434/v1/ ".into(),
            ..Default::default()
        };
        s.normalize();
        assert_eq!(
            (
                s.translate_api_key.as_str(),
                s.translate_model.as_str(),
                s.translate_base_url.as_str()
            ),
            ("sk-1", "gpt", "http://localhost:11434/v1")
        );
        assert!(s.validate().is_ok());
        assert_eq!(
            invalid(&Settings {
                translate_base_url: "api.openai.com".into(),
                ..Default::default()
            }),
            "请输入 http:// 或 https:// 开头的地址"
        );
        // 选了系统翻译时接口三项不显示，不合法的接口地址不拦保存
        assert!(
            Settings {
                translate_engine: "system".into(),
                translate_base_url: "api.openai.com".into(),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
        // 空 = 未配置，不报错
        assert!(
            Settings {
                translate_base_url: String::new(),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
        assert!(
            Settings {
                translate_lang: "fr".into(),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            Settings {
                translate_engine: "deepl".into(),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
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
        assert!(s.ocr_enabled && s.ocr_backfill);
        assert!(!s.search_file_text);
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
                && json.contains(r#""ocrEnabled":true"#)
                && json.contains(r#""ocrBackfill":true"#)
                && json.contains(r#""searchFileText":false"#)
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
    fn default_shot_settings() {
        let s = Settings::default();
        assert_eq!(s.shot_hotkey, "Alt+Shift+A");
        assert_eq!(s.shot_dir, "");
        assert!(s.shot_save_copy);
    }

    #[test]
    fn old_json_gets_shot_defaults() {
        let s: Settings = serde_json::from_str(r#"{"hotkey":"Alt+Space"}"#).unwrap();
        let d = Settings::default();
        assert_eq!(s.shot_hotkey, d.shot_hotkey);
        assert_eq!(s.shot_dir, d.shot_dir);
        assert_eq!(s.shot_save_copy, d.shot_save_copy);
    }

    #[test]
    fn old_json_with_default_shot_combo_disables_shot_hotkey() {
        let mut s: Settings = serde_json::from_str(r#"{"hotkey":"Alt+Shift+A"}"#).unwrap();
        s.disable_clashing_hotkeys();
        assert_eq!(s.shot_hotkey, "");
        assert_eq!(s.hotkey, "Alt+Shift+A");
        assert!(s.validate().is_ok());
        // 不撞时不动
        let mut s = Settings::default();
        s.disable_clashing_hotkeys();
        assert_eq!(s.shot_hotkey, "Alt+Shift+A");
    }

    #[test]
    fn validate_shot_hotkey() {
        let with = |hotkey: &str, shot: &str| Settings {
            hotkey: hotkey.into(),
            shot_hotkey: shot.into(),
            ..Default::default()
        };
        assert!(with("Alt+Space", "").validate().is_ok());
        assert_eq!(
            invalid(&with("Alt+Space", "A")),
            "截图快捷键至少需要包含 Mod 或 Alt"
        );
        assert_eq!(
            invalid(&with("Alt+Shift+A", "alt+shift+a")),
            "截图快捷键不能与唤起快捷键相同"
        );
        assert_eq!(
            invalid(&with("Shift+Alt+A", " Alt + shift+A")),
            "截图快捷键不能与唤起快捷键相同"
        );
        assert_eq!(
            invalid(&with("Option+Shift+A", "Alt+Shift+A")),
            "截图快捷键不能与唤起快捷键相同"
        );
    }

    #[test]
    fn default_rec_settings() {
        let s = Settings::default();
        assert_eq!(s.rec_hotkey, "Alt+Shift+R");
        assert_eq!(s.rec_dir, "");
        assert!(!s.rec_system_audio);
        assert!(!s.rec_microphone);
        assert!(s.rec_copy);
    }

    #[test]
    fn old_json_gets_rec_defaults() {
        let s: Settings = serde_json::from_str(r#"{"hotkey":"Alt+Space"}"#).unwrap();
        let d = Settings::default();
        assert_eq!(s.rec_hotkey, d.rec_hotkey);
        assert_eq!(s.rec_dir, d.rec_dir);
        assert_eq!(s.rec_system_audio, d.rec_system_audio);
        assert_eq!(s.rec_microphone, d.rec_microphone);
        assert_eq!(s.rec_copy, d.rec_copy);
        assert_eq!(s.rec_quality, d.rec_quality);
        assert_eq!(s.rec_fps, d.rec_fps);
    }

    #[test]
    fn rec_quality_and_fps_defaults_and_serde() {
        let d = Settings::default();
        assert_eq!(d.rec_quality, RecQuality::Standard);
        assert_eq!(d.rec_fps, 30);
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains(r#""recQuality":"standard""#), "{json}");
        assert!(json.contains(r#""recFps":30"#), "{json}");
        let s: Settings = serde_json::from_str(r#"{"recQuality":"small","recFps":60}"#).unwrap();
        assert_eq!((s.rec_quality, s.rec_fps), (RecQuality::Small, 60));
        let s: Settings = serde_json::from_str(r#"{"recQuality":"high"}"#).unwrap();
        assert_eq!(s.rec_quality, RecQuality::High);
    }

    #[test]
    fn validate_rec_fps() {
        let with = |fps| Settings {
            rec_fps: fps,
            ..Default::default()
        };
        assert!(with(30).validate().is_ok());
        assert!(with(60).validate().is_ok());
        for fps in [0, 24, 59, 120] {
            assert_eq!(invalid(&with(fps)), "录屏帧率只能是 30 或 60", "{fps}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn validate_rec_hotkey() {
        let with = |hotkey: &str, shot: &str, rec: &str| Settings {
            hotkey: hotkey.into(),
            shot_hotkey: shot.into(),
            rec_hotkey: rec.into(),
            ..Default::default()
        };
        assert!(with("Alt+Space", "Alt+Shift+A", "").validate().is_ok());
        assert_eq!(
            invalid(&with("Alt+Space", "Alt+Shift+A", "R")),
            "录屏快捷键至少需要包含 Mod 或 Alt"
        );
        assert_eq!(
            invalid(&with("Alt+Shift+R", "Alt+Shift+A", " shift+ALT+r")),
            "录屏快捷键不能与唤起快捷键相同"
        );
        assert_eq!(
            invalid(&with("Alt+Space", "Alt+Shift+A", "Option+Shift+A")),
            "录屏快捷键不能与截图快捷键相同"
        );
        assert!(with("Alt+Space", "", "Alt+Shift+A").validate().is_ok());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn validate_ignores_rec_hotkey_off_macos() {
        let with = |hotkey: &str, shot: &str, rec: &str| Settings {
            hotkey: hotkey.into(),
            shot_hotkey: shot.into(),
            rec_hotkey: rec.into(),
            ..Default::default()
        };
        // 默认的 Alt+Shift+R 不占住唤起与截图
        assert!(
            with("Alt+Shift+R", "Alt+Shift+A", "Alt+Shift+R")
                .validate()
                .is_ok()
        );
        assert!(
            with("Alt+Space", "Alt+Shift+R", "Alt+Shift+R")
                .validate()
                .is_ok()
        );
        assert!(with("Alt+Space", "Alt+Shift+A", "R").validate().is_ok());
    }

    #[test]
    fn validate_translate_hotkey() {
        let with = |tr: &str| Settings {
            hotkey: "Alt+Space".into(),
            shot_hotkey: "Alt+Shift+A".into(),
            rec_hotkey: "Alt+Shift+R".into(),
            translate_hotkey: tr.into(),
            ..Default::default()
        };
        assert_eq!(Settings::default().translate_hotkey, "Alt+Shift+T");
        assert!(with("").validate().is_ok());
        assert!(with("Alt+Shift+T").validate().is_ok());
        assert_eq!(invalid(&with("T")), "翻译快捷键至少需要包含 Mod 或 Alt");
        assert_eq!(
            invalid(&with(" alt+SPACE")),
            "翻译快捷键不能与唤起快捷键相同"
        );
        assert_eq!(
            invalid(&with("Option+Shift+A")),
            "翻译快捷键不能与截图快捷键相同"
        );
        // 录屏只在 macOS 上注册（M24），别处不占组合
        let rec = with("shift+Alt+R").validate();
        if cfg!(target_os = "macos") {
            assert_eq!(
                invalid(&with("shift+Alt+R")),
                "翻译快捷键不能与录屏快捷键相同"
            );
        } else {
            assert!(rec.is_ok());
        }
        // 截图不启用时不比
        assert!(
            Settings {
                shot_hotkey: String::new(),
                translate_hotkey: "Alt+Shift+A".into(),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }

    /// M15 翻译快捷键：升级前截图（或唤起、录屏）已用 Alt+Shift+T 的，翻译快捷键读出为不启用，其余设置保留。
    #[test]
    fn translate_hotkey_upgrade_conflict() {
        let (s, dropped) =
            Settings::from_json_lenient(r#"{"shotHotkey":"Alt+Shift+T","maxRecords":1000}"#)
                .unwrap();
        assert_eq!(s.translate_hotkey, "");
        assert_eq!(s.shot_hotkey, "Alt+Shift+T");
        assert_eq!(s.max_records, 1000);
        assert!(dropped.is_empty(), "{dropped:?}");
        let (s, _) = Settings::from_json_lenient(r#"{"hotkey":"Alt+Shift+T"}"#).unwrap();
        assert_eq!(
            (s.hotkey.as_str(), s.translate_hotkey.as_str()),
            ("Alt+Shift+T", "")
        );
        // 录屏撞上：录屏只在 macOS 上注册，别处不让位
        let (s, _) = Settings::from_json_lenient(r#"{"recHotkey":"Alt+Shift+T"}"#).unwrap();
        let want = if cfg!(target_os = "macos") {
            ""
        } else {
            "Alt+Shift+T"
        };
        assert_eq!(s.translate_hotkey, want);
        // 不撞时不动
        let (s, _) = Settings::from_json_lenient(r#"{"maxRecords":1000}"#).unwrap();
        assert_eq!(s.translate_hotkey, "Alt+Shift+T");
        // 已存的、不撞的翻译快捷键保留
        let (s, dropped) = Settings::from_json_lenient(
            r#"{"shotHotkey":"Alt+Shift+T","translateHotkey":"Alt+Shift+U"}"#,
        )
        .unwrap();
        assert_eq!(s.translate_hotkey, "Alt+Shift+U");
        assert!(dropped.is_empty(), "{dropped:?}");
    }

    #[test]
    fn disable_clashing_hotkeys_clears_rec() {
        let mut s = Settings {
            hotkey: "Alt+Shift+R".into(),
            ..Default::default()
        };
        s.disable_clashing_hotkeys();
        assert_eq!(s.rec_hotkey, "");
        assert_eq!(s.shot_hotkey, "Alt+Shift+A");
        let mut s = Settings {
            shot_hotkey: "Alt+Shift+R".into(),
            ..Default::default()
        };
        s.disable_clashing_hotkeys();
        assert_eq!(s.rec_hotkey, "");
        assert_eq!(s.shot_hotkey, "Alt+Shift+R");
        // 不撞时不动
        let mut s = Settings::default();
        s.disable_clashing_hotkeys();
        assert_eq!(s.rec_hotkey, "Alt+Shift+R");
    }

    #[test]
    fn hotkey_key_merges_modifier_aliases_per_platform() {
        use super::hotkey_key_on as key;
        // 两端都一样的别名
        for mac in [true, false] {
            assert_eq!(key("Option+A", mac), key("Alt+A", mac));
            assert_eq!(key("Ctrl+A", mac), key("Control+A", mac));
            assert_eq!(key("Cmd+A", mac), key("Super+A", mac));
            assert_eq!(key("Command+A", mac), key("Meta+A", mac));
            assert_eq!(key("CmdOrCtrl+A", mac), key("CommandOrControl+A", mac));
        }
        // Windows：CommandOrControl 就是 Ctrl，同一个修饰键写两次也只算一次
        assert_eq!(key("CommandOrControl+A", false), key("Ctrl+A", false));
        assert_eq!(
            key("CommandOrControl+Control+A", false),
            key("Ctrl+A", false)
        );
        assert_ne!(key("CommandOrControl+A", false), key("Super+A", false));
        // macOS：CommandOrControl 是 ⌘，与 ⌃ 不同
        assert_eq!(key("CommandOrControl+A", true), key("Cmd+A", true));
        assert_ne!(key("CommandOrControl+A", true), key("Ctrl+A", true));
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
            ocr_state: ocr_state::TEXT,
            snippet_body: None,
        };
        let d = RecordDetail {
            summary,
            text: Some("secret-text".into()),
            truncated: false,
            link_title: None,
            files: vec![],
            image: None,
            video: None,
            ocr_text: Some("secret-ocr".into()),
            translation: None,
            translation_lang: None,
            translation_engine: None,
        };
        let out = format!("{d:?} {:?}", d.summary);
        assert!(!out.contains("secret"), "{out}");
        let json = serde_json::to_string(&d).unwrap();
        assert!(
            json.contains(r#""copiedAt":0"#)
                && json.contains(r#""linkTitle":null"#)
                && json.contains(r#""ocrState":1"#)
                && json.contains(r#""ocrText":"secret-ocr""#),
            "{json}"
        );
    }
}
