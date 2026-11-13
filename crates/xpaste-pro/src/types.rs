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
    /// 提醒时刻（Unix 秒），没有提醒为 None；旧的 line.json 没有这个字段
    #[serde(default)]
    pub remind_at: Option<i64>,
    /// 左右两侧时挂在绳子上的高度：便签离第一张默认挂处往下多少逻辑像素；None 按默认依次往下排。
    /// 顶部时不用；旧的 line.json 没有这个字段
    #[serde(default)]
    pub side_y: Option<f64>,
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
    Trial {
        days_left: u32,
    },
    Licensed {
        email: String,
        /// 买断授权适用的大版本；订阅没有
        major: Option<u32>,
    },
    /// 试用已结束；存着的授权用不了时 `reason` 是原因（凭证过期要联网续期、被授权服务拒绝、只适用于旧的大版本），
    /// 没有激活过为 None
    Expired {
        reason: Option<String>,
    },
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
    /// 排序、摆放带来的 id 集合和现有便签对不上：页面拿的是旧状态
    Stale,
    Io(String),
}

/// 授权服务明确拒绝的原因，和接口返回的 `error` 一一对应
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    Revoked,
    Expired,
    DeviceUnbound,
    InvalidCode,
}

impl Rejection {
    const ALL: [(Self, &str); 4] = [
        (Self::Revoked, "revoked"),
        (Self::Expired, "expired"),
        (Self::DeviceUnbound, "device_unbound"),
        (Self::InvalidCode, "invalid_code"),
    ];

    /// 接口的 `error` 字段；不是拒绝（限流、暂时不可用等）为 None
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, c)| *c == code).map(|(r, _)| *r)
    }

    pub fn code(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(r, _)| *r == self)
            .map_or("", |(_, c)| c)
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Revoked => "这个授权码已停用",
            Self::Expired => "订阅已到期",
            Self::DeviceUnbound => "这台设备已被解绑",
            Self::InvalidCode => "授权码无效",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LicenseError {
    Unavailable,
    /// 授权码的样子不对，没必要去问授权服务
    Format,
    /// 凭证验不过：签名不对、不是这个应用或这台设备的、内容残缺
    Invalid,
    /// 凭证过了有效期，要联网续期
    Stale,
    /// 买断授权的大版本低于应用的大版本，带授权的 major
    OldMajor(u32),
    /// 读不到这台电脑的硬件标识
    Device,
    /// 授权服务明确拒绝
    Rejected(Rejection),
    /// 授权没能写进本机，只带 io 错误信息，不含授权码和邮箱
    Io(String),
}

impl LicenseError {
    /// 给用户看的文案；激活报错和存着的授权用不了共用一套
    pub fn message(&self) -> String {
        match self {
            Self::Unavailable => "这个版本不含晾衣绳".into(),
            Self::Format => "授权码格式不对".into(),
            Self::Invalid => "授权凭证校验失败，请重新激活".into(),
            Self::Stale => "授权已过期，联网后自动续期".into(),
            Self::OldMajor(major) => format!("这个授权码只适用于 {major}.x"),
            Self::Device => "读不到这台电脑的硬件标识，无法激活".into(),
            Self::Rejected(r) => r.message().into(),
            Self::Io(_) => "授权码保存失败，请重试".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejection_codes_round_trip() {
        for (r, c) in Rejection::ALL {
            assert_eq!(Rejection::from_code(c), Some(r));
            assert_eq!(r.code(), c);
        }
        for other in [
            "rate_limited",
            "unavailable",
            "device_limit",
            "not_found",
            "",
        ] {
            assert_eq!(Rejection::from_code(other), None);
        }
    }
}
