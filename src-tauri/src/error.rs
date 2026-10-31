use serde::ser::{Serialize, SerializeStruct, Serializer};
use xpaste_core::CoreError;

/// 命令返回给前端的错误，序列化为 `{ kind, message }`（`translatePack` 另带 `source`、`target`）。
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("原文件已移动或删除，无法粘贴")]
    FileMissing,
    #[error("写入剪切板失败，请重试")]
    ClipboardWrite,
    #[error("该快捷键已被其他应用占用，请换一个")]
    HotkeyTaken,
    #[error("截图快捷键已被其他应用占用，请换一个")]
    ShotHotkeyTaken,
    #[error("录屏快捷键已被其他应用占用，请换一个")]
    RecHotkeyTaken,
    #[error("翻译快捷键已被其他应用占用，请换一个")]
    TranslateHotkeyTaken,
    /// 参数不合法（非 http 链接、记录不存在等）
    #[error("{0}")]
    Invalid(String),
    /// 翻译接口未配置（M26 规则 13）
    #[error("请先在设置的“翻译”里配置接口")]
    TranslateConfig,
    /// 系统翻译缺语言包，消息为拼好的提示（M26 规则 13）；`from` / `to` 为源、目标语言代码，
    /// 序列化为 `source` / `target`，面板据此下载语言包并重试（M26 规则 17）。字段不叫 source：thiserror 会当成错误源
    #[error("{msg}")]
    TranslatePack {
        msg: String,
        from: String,
        to: String,
    },
    #[error("系统翻译需要 macOS 26，请在设置里改用 OpenAI 兼容接口")]
    TranslateOs,
    /// 其余翻译错误，消息见 M26 规则 13
    #[error("{0}")]
    Translate(String),
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error(transparent)]
    Tauri(#[from] tauri::Error),
    #[error(transparent)]
    Opener(#[from] tauri_plugin_opener::Error),
    /// 截图与贴图读写文件；`io::Error` 的消息不含路径
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl AppError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::FileMissing => "fileMissing",
            Self::ClipboardWrite => "clipboardWrite",
            Self::HotkeyTaken => "hotkeyTaken",
            Self::ShotHotkeyTaken => "shotHotkeyTaken",
            Self::RecHotkeyTaken => "recHotkeyTaken",
            Self::TranslateHotkeyTaken => "translateHotkeyTaken",
            Self::Invalid(_) => "invalid",
            Self::TranslateConfig => "translateConfig",
            Self::TranslatePack { .. } => "translatePack",
            Self::TranslateOs => "translateOs",
            Self::Translate(_) => "translate",
            Self::Core(CoreError::InvalidSettings(_)) => "invalidSettings",
            Self::Core(_) | Self::Tauri(_) | Self::Opener(_) | Self::Io(_) => "internal",
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("AppError", 4)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.to_string())?;
        if let Self::TranslatePack { from, to, .. } = self {
            s.serialize_field("source", from)?;
            s.serialize_field("target", to)?;
        }
        s.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serializes_as_kind_and_message() {
        assert_eq!(
            serde_json::to_value(AppError::FileMissing).unwrap(),
            json!({"kind": "fileMissing", "message": "原文件已移动或删除，无法粘贴"})
        );
        assert_eq!(
            serde_json::to_value(AppError::HotkeyTaken).unwrap()["kind"],
            "hotkeyTaken"
        );
        assert_eq!(
            serde_json::to_value(AppError::ShotHotkeyTaken).unwrap(),
            json!({"kind": "shotHotkeyTaken", "message": "截图快捷键已被其他应用占用，请换一个"})
        );
        assert_eq!(
            serde_json::to_value(AppError::RecHotkeyTaken).unwrap(),
            json!({"kind": "recHotkeyTaken", "message": "录屏快捷键已被其他应用占用，请换一个"})
        );
        assert_eq!(
            serde_json::to_value(AppError::TranslateHotkeyTaken).unwrap(),
            json!({"kind": "translateHotkeyTaken", "message": "翻译快捷键已被其他应用占用，请换一个"})
        );
        assert_eq!(
            serde_json::to_value(AppError::ClipboardWrite).unwrap()["message"],
            "写入剪切板失败，请重试"
        );
    }

    /// 缺语言包带回源、目标语言代码，面板据此下载并重试（M26 规则 17）。
    #[test]
    fn translate_pack_serializes_languages() {
        let e = AppError::TranslatePack {
            msg: "系统翻译还没有English和简体中文的语言包".into(),
            from: "en".into(),
            to: "zh-Hans".into(),
        };
        assert_eq!(
            serde_json::to_value(e).unwrap(),
            json!({"kind": "translatePack", "message": "系统翻译还没有English和简体中文的语言包", "source": "en", "target": "zh-Hans"})
        );
        assert!(
            serde_json::to_value(AppError::TranslateOs)
                .unwrap()
                .get("source")
                .is_none()
        );
    }

    #[test]
    fn invalid_serializes_with_kind() {
        assert_eq!(
            serde_json::to_value(AppError::Invalid("x".into())).unwrap(),
            json!({"kind": "invalid", "message": "x"})
        );
    }

    #[test]
    fn core_invalid_settings_keeps_its_kind() {
        let e: AppError =
            xpaste_core::CoreError::InvalidSettings("至少需要包含 Mod 或 Alt".into()).into();
        assert_eq!(
            serde_json::to_value(e).unwrap(),
            json!({"kind": "invalidSettings", "message": "至少需要包含 Mod 或 Alt"})
        );
    }

    #[test]
    fn core_error_other_maps_to_internal() {
        let e: AppError = CoreError::Db(rusqlite::Error::QueryReturnedNoRows).into();
        let v = serde_json::to_value(e).unwrap();
        assert_eq!(v["kind"], "internal");
        assert!(!v["message"].as_str().unwrap().is_empty());
    }
}
