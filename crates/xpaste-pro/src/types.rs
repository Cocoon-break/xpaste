//! Pro 对外的数据类型，stub 和私有实现共用。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
pub use xpaste_core::Kind;

/// 挂在晾衣绳上的一张便签。不派生 `Debug`：里面有正文。
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: u64,
    pub kind: Kind,
    /// 文字、代码的正文；链接的网址；图片、视频、文件的名字
    pub body: String,
    /// 链接标题
    pub title: Option<String>,
    pub memo: Option<String>,
    /// `line/` 下的文件名（图片原图、视频封面）
    pub image: Option<String>,
    pub size: Option<u64>,
    pub file_kind: Option<String>,
    pub duration_sec: Option<f64>,
    /// 文件、视频的源路径
    pub paths: Vec<String>,
    pub record_id: Option<i64>,
    pub source: Option<String>,
    pub created_at: i64,
}

/// 新挂的便签：`Note` 去掉 id、创建时间和图片文件名，加上要复制进 `line/` 的源文件。不派生 `Debug`：里面有正文。
#[derive(Clone)]
pub struct NewNote {
    pub kind: Kind,
    pub body: String,
    pub title: Option<String>,
    pub memo: Option<String>,
    pub size: Option<u64>,
    pub file_kind: Option<String>,
    pub duration_sec: Option<f64>,
    pub paths: Vec<String>,
    pub record_id: Option<i64>,
    pub source: Option<String>,
    /// 要复制进 `line/` 的源文件
    pub image_from: Option<PathBuf>,
}

/// Pro 状态
#[derive(Clone, Serialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Pro {
    Unavailable,
    Trial { days_left: u32 },
    Licensed { email: String, major: u32 },
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineError {
    Unavailable,
    Expired,
    Full,
    /// 同一条内容已挂着，带已有便签的 id
    Duplicate(u64),
    TooLarge,
    NotFound,
    Io(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LicenseError {
    Unavailable,
    Format,
    Invalid,
    Revoked,
    /// 授权码的大版本低于应用的大版本，带授权码的 major
    OldMajor(u32),
}
