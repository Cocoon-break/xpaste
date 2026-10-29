use serde::ser::{Serialize, SerializeStruct, Serializer};
use xpaste_core::CoreError;

/// 命令返回给前端的错误，序列化为 `{ kind, message }`。
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
    /// 参数不合法（非 http 链接、记录不存在等）
    #[error("{0}")]
    Invalid(String),
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
            Self::Invalid(_) => "invalid",
            Self::Core(CoreError::InvalidSettings(_)) => "invalidSettings",
            Self::Core(_) | Self::Tauri(_) | Self::Opener(_) | Self::Io(_) => "internal",
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("AppError", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.to_string())?;
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
            serde_json::to_value(AppError::ClipboardWrite).unwrap()["message"],
            "写入剪切板失败，请重试"
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
