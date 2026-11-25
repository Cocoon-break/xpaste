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

/// 传送带上最多放这么多件
pub const BELT_MAX: usize = 100;

/// 带子上的一件。不派生 `Debug`：里面有正文。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BeltItem {
    pub id: u64,
    pub kind: Kind,
    pub title: String,
    pub record_id: Option<i64>,
    /// 文字、链接、代码和拆出来的件放全文；图片、文件、视频为 None，只记 `record_id`
    #[serde(skip)]
    pub text: Option<String>,
}

/// 一段文字拆开的结果；`cells` 为 true 表示按制表符分的格，否则按行
pub struct Split {
    pub parts: Vec<String>,
    pub cells: bool,
}

#[derive(Debug, PartialEq)]
pub enum BeltError {
    Full,
    NotFound,
}

/// 拿掉的一件和它的位置，撤销时放回去
// 空实现不读这些字段
#[cfg_attr(not(pro), allow(dead_code))]
pub struct Removed {
    pub(crate) item: BeltItem,
    pub(crate) at: usize,
    pub(crate) was_next: bool,
}

/// 清空前的全部件，撤销时放回去
// 空实现不读这些字段
#[cfg_attr(not(pro), allow(dead_code))]
pub struct Cleared {
    pub(crate) items: Vec<BeltItem>,
    pub(crate) next: Option<u64>,
}

/// 杯子最多放这么多样
pub const CUP_MAX: usize = 8;
/// 文字原料加补一句的总字数上限
pub const MAX_CHARS: usize = 20_000;
/// 配方个数上限
pub const RECIPES_MAX: usize = 30;

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IngredientKind {
    Text,
    Link,
    Code,
    Image,
    File,
}

/// 原料的内容：文字类存全文；图片只记记录 id，图片文件另记路径（开搅时才读）
#[derive(Clone)]
pub enum Body {
    Text(String),
    Image {
        record_id: i64,
        path: Option<PathBuf>,
    },
}

/// 杯子里的一样。不派生 `Debug`：里面有正文。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ingredient {
    pub id: u64,
    pub kind: IngredientKind,
    pub title: String,
    pub record_id: Option<i64>,
    /// 悬停提示用：文字类的前三行（最多 200 字），图片为空串
    pub preview: String,
    #[serde(skip)]
    pub body: Body,
}

#[derive(Debug, PartialEq)]
pub enum CupError {
    Full,
    NotFound,
}

/// 拿掉的一样和它的位置，撤销时放回去。不派生 `Debug`：里面有正文。
// 空实现不读这些字段
#[cfg_attr(not(pro), allow(dead_code))]
pub struct Taken {
    pub(crate) item: Ingredient,
    pub(crate) at: usize,
}

/// 请求里的一段材料。不派生 `Debug`：里面有正文。
pub enum Material {
    /// `label` 是括号里的类别：“文字”“链接”“代码”“文件 名字”
    Text {
        label: String,
        text: String,
        code: bool,
    },
    Image {
        data_url: String,
    },
}

/// 内置风格：键名、显示名、类别
pub struct MemeStyle {
    pub key: &'static str,
    pub name: &'static str,
    pub cat: &'static str,
}

/// 一次生图要什么：一张、一套 16 张（4 × 4 网格）、分 4 次里的第几次（2 × 2），或同一角色的 4 帧动作
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MemeGen {
    One,
    Set,
    /// 0..4
    SetPart(u8),
    Act(MemeAct),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MemeAct {
    Blink,
    Nod,
    Wave,
    Cry,
}

/// 生图提示词的材料。不派生 `Debug`：里面有用户写的话。
pub struct MemePrompt<'a> {
    pub style_key: Option<&'a str>,
    pub custom_style: Option<&'a str>,
    pub idea: &'a str,
    pub caption: &'a str,
    pub r#gen: MemeGen,
    /// 草图以外的参考图有几张（不算 `anchor`）
    pub photos: usize,
    pub has_sketch: bool,
    /// 分次重画时，最后一张参考图是这一套里已经画好的一格
    pub anchor: bool,
}

/// 导出 GIF 的一档：帧间隔、颜色数、边长
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExportStep {
    pub delay_ms: u16,
    pub colors: u16,
    pub side: u32,
}

/// 一张解码好的图，RGBA8，行优先；`px.len() == w * h * 4`，宽高至少为 1。不派生 `Debug`：里面是用户的图。
pub struct TeImg {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

/// 一行字的四角，原图的连续坐标（像素 (i, j) 占 [i, i+1) × [j, j+1)），
/// 顺序是沿文字方向的左上、右上、右下、左下
pub type TeQuad = [[f32; 2]; 4];

/// 从图里裁出的 1024 × 1024 正方形和它的遮罩。不派生 `Debug`：里面是用户的图。
pub struct TeCrop {
    /// RGB PNG
    pub png: Vec<u8>,
    /// RGBA PNG，透明处为要改的
    pub mask: Vec<u8>,
    /// 原图 → 裁块的单应矩阵，行优先；两边都是连续坐标
    pub m: [f64; 9],
}

/// 一行字拉正后的条带，正好是四角围住的那块
pub struct TeStrip {
    pub img: TeImg,
    /// 原图 → 条带的单应矩阵，行优先；两边都是连续坐标
    pub m: [f64; 9],
}

/// 要覆盖到原图 `(x, y)` 的矩形，含没变的像素
pub struct TePaste {
    pub patch: TeImg,
    pub x: u32,
    pub y: u32,
    /// 改动范围外没动过，直接取的像素
    pub exact: bool,
    /// 可能有痕迹
    pub soft: bool,
}
