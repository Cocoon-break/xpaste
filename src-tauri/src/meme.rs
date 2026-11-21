//! 表情包制作：总开关、入口（面板笑脸、`Mod+E`、全局快捷键、托盘）与制作窗口的运行时（只在带 Pro 的构建里启用）。
//!
//! 参考图由页面持有，这里记一份镜像（打开时带进去的、页面报来的），再从面板带记录进来时据此判断还能加几张。

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use xpaste_core::Kind;

use crate::error::AppError;
use crate::state::AppState;

pub const LABEL: &str = "meme";
/// 参考图最多几张
pub const REFS_MAX: usize = 8;

const UNUSABLE: &str = "这几条做不了表情包";
pub const FULL: &str = "参考图满了（8 张）";

/// 表情包制作是否编入此构建（与搅拌机同一个条件）。设置里的总开关由调用方另查（`on`）。
pub fn enabled() -> bool {
    crate::line::enabled()
}

/// 编入了、并且设置里开着
fn on(app: &AppHandle) -> bool {
    enabled() && app.state::<AppState>().settings().meme_enabled
}

/// 读入设置、保存设置时整理风格库：去重、去掉坏项（规则在 Pro 里，不含 Pro 的构建不动）。
pub fn normalize_settings(settings: &mut xpaste_core::Settings) {
    if enabled() {
        let (used, custom) = xpaste_pro::meme_normalize_styles(
            std::mem::take(&mut settings.meme_styles_used),
            std::mem::take(&mut settings.meme_styles_custom),
        );
        settings.meme_styles_used = used;
        settings.meme_styles_custom = custom;
    }
}

/// 一张参考图：历史记录的 id，文件记录里是第几个文件（位图记录为 None）。只记不复制，用到时才读。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RefKey {
    pub id: i64,
    pub file: Option<u32>,
}

/// 发给页面的 `MEME_OPEN`。带着想表达什么，不派生 Debug。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenPayload {
    /// 打开后窗口里的全部参考图（已开着时是追加之后的）
    pub refs: Vec<RefKey>,
    /// 页面只在“想表达什么”为空时放进去
    pub idea: String,
    pub scene: Option<String>,
    pub draft: Option<String>,
    /// 本机能抠主体
    pub cutout: bool,
    /// 试用已结束：不带参考图和文字，页面只显示过期提示
    pub expired: bool,
}

/// 选中的一条记录里做表情包用得上的部分
pub enum Item {
    Image(i64),
    /// 文件记录和它的文件（按文件拆，只取图片文件）
    Files(i64, Vec<PathBuf>),
    Text(String),
    /// 视频、链接、代码、片段、表情包、已删除
    Skip,
}

/// 选中的记录 → 参考图和“想表达什么”（第一条文字）。全都用不上时返回给用户看的原因。
pub fn from_selection(items: Vec<Item>) -> Result<(Vec<RefKey>, String), &'static str> {
    let (mut refs, mut idea) = (Vec::new(), None);
    for it in items {
        match it {
            Item::Image(id) => refs.push(RefKey { id, file: None }),
            Item::Files(id, paths) => refs.extend(
                paths
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| xpaste_core::detect::is_image_file(p))
                    .map(|(i, _)| RefKey {
                        id,
                        file: Some(i as u32),
                    }),
            ),
            Item::Text(t) if !t.trim().is_empty() => {
                idea.get_or_insert(t);
            }
            Item::Text(_) | Item::Skip => {}
        }
    }
    if refs.is_empty() && idea.is_none() {
        return Err(UNUSABLE);
    }
    Ok((refs, idea.unwrap_or_default()))
}

/// 已有的参考图后面追加，满 8 张的部分不加；第二项为有没加进去的。
pub fn merge_refs(have: &[RefKey], add: Vec<RefKey>) -> (Vec<RefKey>, bool) {
    let room = REFS_MAX.saturating_sub(have.len());
    let full = add.len() > room;
    let mut refs = have.to_vec();
    refs.extend(add.into_iter().take(room));
    (refs, full)
}

/// 这次打开窗口里放什么：试用结束时什么都不带（页面只显示过期提示，已有的参考图不动），
/// 否则参考图追加到已有的后面。返回参考图、想表达什么、有没有因为满了没加进去的。
pub fn opening(
    expired: bool,
    have: &[RefKey],
    add: Vec<RefKey>,
    idea: String,
) -> (Vec<RefKey>, String, bool) {
    if expired {
        return (Vec::new(), String::new(), false);
    }
    let (refs, full) = merge_refs(have, add);
    (refs, idea, full)
}

struct Runtime {
    open: bool,
    /// 窗口里此刻的参考图（镜像）
    refs: Vec<RefKey>,
    /// 打开前的前台窗口（只有 Windows 有），关掉时交还
    back: Option<isize>,
}

static RT: Mutex<Runtime> = Mutex::new(Runtime {
    open: false,
    refs: Vec::new(),
    back: None,
});

fn rt() -> MutexGuard<'static, Runtime> {
    RT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 读出选中的记录（不要在持有 `RT` 时调用）。已删的、读不出来的当作用不上。
fn gather(state: &AppState, ids: &[i64]) -> Vec<Item> {
    let store = state.store();
    ids.iter()
        .map(|&id| {
            let s = match store.live_summary(id) {
                Ok(Some(s)) if s.snippet_body.is_none() && s.meme.is_none() => s,
                Ok(_) => return Item::Skip,
                Err(e) => {
                    log::warn!("表情包 读取记录 {id} 失败：{e}");
                    return Item::Skip;
                }
            };
            match s.kind {
                Kind::Image => Item::Image(id),
                Kind::Text => store.text(id).ok().flatten().map_or(Item::Skip, Item::Text),
                Kind::File => store.detail(id).ok().flatten().map_or(Item::Skip, |d| {
                    Item::Files(id, d.files.into_iter().map(|f| f.path.into()).collect())
                }),
                Kind::Code | Kind::Link | Kind::Video => Item::Skip,
            }
        })
        .collect()
}

/// 带着这些记录打开制作窗口（`ids` 为空 = 空白打开）。已开着时参考图追加、满了提示；面板开着就收起。
/// 试用结束时照常打开，载荷标上过期、不带记录，页面只显示过期提示。选中的全都用不上时返回原因，面板提示。
pub fn open(app: &AppHandle, ids: &[i64]) -> Result<(), AppError> {
    if !on(app) {
        return Ok(());
    }
    let expired = !crate::mixer::accepts_new(&crate::mixer::pro(app));
    let (add, idea) = if ids.is_empty() || expired {
        (Vec::new(), String::new())
    } else {
        from_selection(gather(&app.state::<AppState>(), ids))
            .map_err(|e| AppError::Invalid(e.into()))?
    };
    let (refs, idea, full, fresh) = {
        let mut rt = rt();
        let fresh = !rt.open;
        if fresh {
            rt.refs.clear();
            rt.back = xpaste_platform::foreground_window();
        }
        let (refs, idea, full) = opening(expired, &rt.refs, add, idea);
        if !expired {
            rt.refs.clone_from(&refs);
        }
        rt.open = true;
        (refs, idea, full, fresh)
    };
    log::info!(
        "表情包 打开 {} 参考图 {} 张{}{}",
        if fresh { "新窗口" } else { "追加" },
        refs.len(),
        if full { "（满了）" } else { "" },
        if expired { "（试用已结束）" } else { "" }
    );
    let payload = OpenPayload {
        refs,
        idea,
        scene: None,
        draft: None,
        cutout: xpaste_platform::foreground_supported(),
        expired,
    };
    if crate::panel::visible(app) {
        crate::panel::hide(app);
    }
    crate::meme_window::show(app, payload, fresh, full);
    Ok(())
}

/// 关掉制作窗口（Esc、关闭按钮、Alt+F4、总开关关掉）。没开着时不做事。
pub fn close(app: &AppHandle) {
    let back = {
        let mut rt = rt();
        if !rt.open {
            return;
        }
        rt.open = false;
        rt.refs.clear();
        rt.back.take()
    };
    // 进行中的生成请求在接上生成之后一并中止
    log::info!("表情包 关闭窗口");
    crate::meme_window::hide(app, back);
}

/// 页面的参考图变了（拿掉、从历史添加、画草图）：更新镜像。
pub fn set_refs(refs: Vec<RefKey>) {
    let mut rt = rt();
    if rt.open {
        rt.refs = refs;
    }
}

/// 表情包快捷键按下（主线程）：面板在屏上且拿着键盘时交给面板（录制框接收这个组合，否则面板带着选中的打开）；
/// 否则空白打开制作窗口。
pub fn hotkey_pressed(app: &AppHandle) {
    if crate::panel::visible(app) && crate::panel::holds_key(app) {
        crate::events::emit_to(app, "panel", crate::events::HOTKEY_PRESSED, "meme");
        return;
    }
    // 空白打开不读库，也不会失败
    let _ = open(app, &[]);
}

/// 最近 8 张能当参考图的（“从历史添加”）：图片记录，以及图片文件记录里的图片文件（按文件拆开）。
pub fn recent_images(state: &AppState) -> Result<Vec<RefKey>, AppError> {
    let store = state.store();
    let mut out = Vec::new();
    // ponytail: 读出全部摘要再挑，同 mixer_recent；记录多到明显变慢时换成带 LIMIT 的查询
    for s in store.list_summaries()? {
        if out.len() >= REFS_MAX {
            break;
        }
        if s.snippet_body.is_some() || s.meme.is_some() {
            continue;
        }
        match s.kind {
            Kind::Image => out.push(RefKey {
                id: s.id,
                file: None,
            }),
            Kind::File => {
                let paths = store
                    .detail(s.id)?
                    .map(|d| d.files.into_iter().map(|f| f.path.into()).collect())
                    .unwrap_or_default();
                if let Ok((refs, _)) = from_selection(vec![Item::Files(s.id, paths)]) {
                    out.extend(refs);
                }
            }
            _ => {}
        }
    }
    out.truncate(REFS_MAX);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(id: i64, file: Option<u32>) -> RefKey {
        RefKey { id, file }
    }

    #[test]
    fn open_payload_from_selection() {
        let items = vec![
            Item::Image(1),
            Item::Text("不想上班".into()),
            Item::Skip, // 链接
            Item::Files(4, vec!["a.png".into(), "b.PNG".into(), "c.pdf".into()]),
        ];
        let (refs, idea) = from_selection(items).unwrap();
        assert_eq!(refs, vec![r(1, None), r(4, Some(0)), r(4, Some(1))]);
        assert_eq!(idea, "不想上班");
    }

    #[test]
    fn open_payload_first_text_only() {
        let items = vec![
            Item::Text("  ".into()),
            Item::Text("甲".into()),
            Item::Text("乙".into()),
        ];
        assert_eq!(from_selection(items).unwrap(), (vec![], "甲".into()));
    }

    #[test]
    fn open_payload_all_unusable() {
        // 只有链接和视频；只有 PDF 的文件记录也一样
        assert_eq!(
            from_selection(vec![Item::Skip, Item::Skip]).err(),
            Some(UNUSABLE)
        );
        assert_eq!(
            from_selection(vec![Item::Files(2, vec!["x.pdf".into()])]).err(),
            Some(UNUSABLE)
        );
    }

    #[test]
    fn expired_opens_without_refs() {
        let have = vec![r(1, None)];
        assert_eq!(
            opening(true, &have, vec![r(2, None)], "不想上班".into()),
            (vec![], String::new(), false)
        );
        assert_eq!(
            opening(false, &have, vec![r(2, None)], "不想上班".into()),
            (vec![r(1, None), r(2, None)], "不想上班".into(), false)
        );
    }

    #[test]
    fn refs_capped_at_8() {
        let have: Vec<_> = (1..=7).map(|i| r(i, None)).collect();
        let (refs, full) = merge_refs(&have, vec![r(8, None), r(9, None), r(10, None)]);
        assert_eq!(refs.len(), 8);
        assert_eq!(refs[7], r(8, None));
        assert!(full);
        let (refs, full) = merge_refs(&[], vec![r(1, None)]);
        assert_eq!((refs.len(), full), (1, false));
    }
}
