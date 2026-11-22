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
    /// 接着改：做法 JSON 和它是库里哪一个（复制时做法没变就不重复进库）
    pub scene: Option<String>,
    pub scene_id: Option<i64>,
    /// 接着改的是做好的成品：做法没变时复制只放剪切板。AI 原图不算（复制要导出 240 的成品）
    pub scene_final: bool,
    /// 空白打开新窗口时的草稿：页面画成一张卡片，点“接着做”才恢复
    pub draft: Option<String>,
    /// 本机能抠主体
    pub cutout: bool,
    /// 试用已结束：不带参考图和文字，页面只显示过期提示
    pub expired: bool,
    /// 回到 AI 任务的那次：任务号（`scene` 是任务里的做法，`scene_id` 为 None）；平时为 None
    pub task_id: Option<u64>,
    /// 那次已经画好的结果 id（网格没画规整的是整张那一个）；在画、排队、没画出来的为空
    pub task_ids: Vec<i64>,
    /// 那次的种类（one | set | act）；不是回到任务时为 None
    pub task_kind: Option<&'static str>,
    /// 那次网格没画规整（`task_ids` 是整张那一个）
    pub task_irregular: bool,
    /// 粘贴目标：打开窗口时前台应用的名字（主按钮写“贴到<它>”）；从托盘打开、取不到名字时为 None
    pub target: Option<String>,
    /// 这次窗口是从面板里打开的：标题前有“返回面板”，Esc 回面板
    pub from_panel: bool,
}

/// 从哪打开的
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// 面板的笑脸、`Mod+E`、“接着改”、“＋ 做一个”、面板里的任务格
    Panel,
    /// 托盘菜单：没有粘贴目标（同面板从托盘唤起）
    Tray,
    /// 全局快捷键等其余入口
    Other,
}

/// 打开窗口时记下的来处，关窗口时用
#[derive(Clone, Debug, PartialEq)]
pub struct Origin {
    /// 打开前的前台窗口（只有 Windows 有），关掉时交还、贴过去时按它粘贴
    pub back: Option<isize>,
    /// 粘贴目标的应用名
    pub target: Option<String>,
    pub from_panel: bool,
    /// 从托盘打开的：一直没有粘贴目标
    pub tray: bool,
}

const NOWHERE: Origin = Origin {
    back: None,
    target: None,
    from_panel: false,
    tray: false,
};

/// 这次打开之后的来处。新窗口按入口记下前台（`fg`：前台窗口和应用名；托盘打开不取，没有目标）；
/// 已开着时目标不变，从面板带进来时记成从面板来的，快捷键、托盘再打开不改。
pub fn arrive(
    had: Option<Origin>,
    entry: Entry,
    fg: impl FnOnce() -> (Option<isize>, Option<String>),
) -> Origin {
    let panel = entry == Entry::Panel;
    match had {
        Some(o) => Origin {
            from_panel: o.from_panel || panel,
            ..o
        },
        None if entry == Entry::Tray => Origin {
            tray: true,
            ..NOWHERE
        },
        None => {
            let (back, target) = fg();
            Origin {
                back,
                target,
                from_panel: panel,
                tray: false,
            }
        }
    }
}

/// 窗口重新拿到焦点时的粘贴目标（`front`：此刻前台应用的名字，前台是 xPaste 自己时为 None）。
/// `follow`（macOS）：贴的时候按给此刻最前面的应用，目标跟着换成它；从托盘打开的照旧没有目标。
/// 不跟（Windows）：贴回打开时记下的那个窗口，不改
pub fn retarget(
    o: &mut Origin,
    follow: bool,
    front: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if follow && !o.tray {
        o.target = front();
    }
    o.target.clone()
}

/// 制作窗口重新拿到焦点：更新并返回粘贴目标的名字；窗口没开着时为 None
pub fn refresh_target() -> Option<String> {
    let mut rt = rt();
    if !rt.open {
        return None;
    }
    retarget(&mut rt.origin, cfg!(target_os = "macos"), || {
        target_name(
            xpaste_platform::frontmost_app(),
            xpaste_platform::frontmost_is_self(),
        )
    })
}

/// 此刻的前台窗口和前台应用的名字（同采集时的来源应用名）
fn foreground() -> (Option<isize>, Option<String>) {
    let back = xpaste_platform::foreground_window();
    // Windows 的 frontmost_app 先看剪切板所有者，这里要的是前台窗口所属的应用（已排除本进程）
    #[cfg(windows)]
    let name = back.and_then(xpaste_platform::window_app_name);
    #[cfg(not(windows))]
    let name = target_name(
        xpaste_platform::frontmost_app(),
        xpaste_platform::frontmost_is_self(),
    );
    (back, name)
}

/// 粘贴目标的名字：前台是 xPaste 自己（比如设置窗口拿着键盘）时没有目标，贴不回自己
fn target_name(front: Option<xpaste_core::AppInfo>, is_self: bool) -> Option<String> {
    front.filter(|_| !is_self).map(|a| a.name)
}

/// 选中的一条记录里做表情包用得上的部分
pub enum Item {
    Image(i64),
    /// 带文件的图片、文件记录和它的文件（按文件拆，只取图片文件）
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
    origin: Origin,
}

static RT: Mutex<Runtime> = Mutex::new(Runtime {
    open: false,
    refs: Vec::new(),
    origin: NOWHERE,
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
                Kind::Image | Kind::File => media(&store, id, s.kind),
                Kind::Text => store.text(id).ok().flatten().map_or(Item::Skip, Item::Text),
                Kind::Code | Kind::Link | Kind::Video => Item::Skip,
            }
        })
        .collect()
}

/// 图片、文件记录里的图：带文件的（复制的图片文件也存成图片记录）按文件拆，位图记录是它自己。
/// 图片文件存成的图片记录没有 `images/<id>.png`，只能按文件读
fn media(store: &xpaste_core::Store, id: i64, kind: Kind) -> Item {
    let paths: Vec<PathBuf> = store
        .detail(id)
        .ok()
        .flatten()
        .map(|d| d.files.into_iter().map(|f| f.path.into()).collect())
        .unwrap_or_default();
    match kind {
        Kind::Image if paths.is_empty() => Item::Image(id),
        _ => Item::Files(id, paths),
    }
}

/// 带着这些记录打开制作窗口（`ids` 为空 = 空白打开，新窗口时带上草稿）。已开着时参考图追加、满了提示；面板开着就收起。
/// 试用结束时照常打开，载荷标上过期、不带记录，页面只显示过期提示。选中的全都用不上时返回原因，面板提示。
pub fn open(app: &AppHandle, ids: &[i64], entry: Entry) -> Result<(), AppError> {
    open_with(app, ids, None, None, entry).map(|_| ())
}

/// 接着改：按库里这个表情包的做法打开（做法坏了：AI 出的只带它自己，做好的空白打开）。不恢复草稿，草稿也不丢
pub fn open_scene(app: &AppHandle, id: i64) -> Result<(), AppError> {
    if !on(app) {
        return Ok(());
    }
    let (scene, ai) = {
        let state = app.state::<AppState>();
        let store = state.store();
        let Some(m) = store.live_summary(id)?.and_then(|s| s.meme) else {
            return Err(AppError::Invalid("这个表情包已经不在了".into()));
        };
        (store.meme_scene(id)?, m.source == "ai")
    };
    open_with(
        app,
        &[],
        Some((
            crate::meme_store::scene_for_open(scene, id, ai),
            Some(id),
            !ai,
        )),
        None,
        Entry::Panel,
    )
    .map(|_| ())
}

/// 回到 AI 任务的那次：按任务里的做法打开，带上任务号和已画好的结果。返回是不是真恢复了（没关开关、试用没结束）
pub fn open_task(
    app: &AppHandle,
    task: &crate::meme_tasks::TaskView,
    ids: Vec<i64>,
) -> Result<bool, AppError> {
    // 面板里的任务格算从面板来；窗口自己的任务列表、系统通知不算
    let entry = if crate::panel::visible(app) {
        Entry::Panel
    } else {
        Entry::Other
    };
    open_with(
        app,
        &[],
        Some((Some(task.scene.clone()), None, false)),
        Some((task.id, ids, task.kind, task.irregular.is_some())),
        entry,
    )
}

/// 制作窗口开着没有
pub fn window_open() -> bool {
    rt().open
}

/// `again`：接着改的做法（可能没有）、库里的 id（回到任务时没有）、是不是做好的成品；
/// `task`：回到 AI 任务时的任务号、结果、种类和是不是没画规整。返回是不是按 `again` 恢复了（试用结束、开关关着时不是）
fn open_with(
    app: &AppHandle,
    ids: &[i64],
    again: Option<(Option<String>, Option<i64>, bool)>,
    task: Option<(u64, Vec<i64>, &'static str, bool)>,
    entry: Entry,
) -> Result<bool, AppError> {
    if !on(app) {
        return Ok(false);
    }
    let expired = !crate::mixer::accepts_new(&crate::mixer::pro(app));
    let (add, idea) = if ids.is_empty() || expired {
        (Vec::new(), String::new())
    } else {
        from_selection(gather(&app.state::<AppState>(), ids))
            .map_err(|e| AppError::Invalid(e.into()))?
    };
    let (refs, idea, full, fresh, origin) = {
        let mut rt = rt();
        let fresh = !rt.open;
        let had = (!fresh).then(|| rt.origin.clone());
        rt.origin = arrive(had, entry, foreground);
        // 接着改换掉窗口里的全部（参考图由页面恢复后报来）
        if fresh || again.is_some() {
            rt.refs.clear();
        }
        let (refs, idea, full) = opening(expired, &rt.refs, add, idea);
        if !expired {
            rt.refs.clone_from(&refs);
        }
        rt.open = true;
        (refs, idea, full, fresh, rt.origin.clone())
    };
    let blank = ids.is_empty() && again.is_none() && fresh && !expired;
    let draft = blank
        .then(|| crate::meme_store::draft_get(&app.state::<AppState>().data_dir))
        .flatten();
    let restored = again.is_some() && !expired;
    let (scene, scene_id, scene_final) = match again.filter(|_| !expired) {
        Some((s, id, fin)) => (s, id, fin),
        None => (None, None, false),
    };
    let (task_id, task_ids, task_kind, task_irregular) = match task.filter(|_| !expired) {
        Some((t, ids, kind, irr)) => (Some(t), ids, Some(kind), irr),
        None => (None, Vec::new(), None, false),
    };
    log::info!(
        "表情包 打开 {} 参考图 {} 张{}{}{}{}",
        if fresh { "新窗口" } else { "追加" },
        refs.len(),
        match (origin.from_panel, origin.target.is_some()) {
            (true, true) => "（从面板，有粘贴目标）",
            (true, false) => "（从面板）",
            (false, true) => "（有粘贴目标）",
            (false, false) => "",
        },
        if full { "（满了）" } else { "" },
        if expired { "（试用已结束）" } else { "" },
        match (scene_id, task_id, draft.is_some()) {
            (Some(id), _, _) => format!("（接着改 {id}）"),
            (None, Some(t), _) => format!("（回到任务 {t}）"),
            (None, None, true) => "（有草稿）".into(),
            _ => String::new(),
        }
    );
    let payload = OpenPayload {
        refs,
        idea,
        scene,
        scene_id,
        scene_final,
        draft,
        cutout: xpaste_platform::foreground_supported(),
        expired,
        task_id,
        task_ids,
        task_kind,
        task_irregular,
        target: origin.target,
        from_panel: origin.from_panel,
    };
    if crate::panel::visible(app) {
        crate::panel::hide(app);
    }
    crate::meme_window::show(app, payload, fresh, full);
    Ok(restored)
}

/// 关掉制作窗口（关闭按钮、复制完、Alt+F4、总开关关掉），键盘回到原来的应用。没开着时不做事。
pub fn close(app: &AppHandle) {
    if let Some(o) = shut() {
        log::info!("表情包 关闭窗口");
        crate::meme_window::hide(app, o.back);
    }
}

/// 存进表情包之后：关掉窗口，唤起面板切到“表情包”分类、选中刚存的这个。
/// 面板的粘贴目标是打开制作窗口前的应用，选中后按 ↩ 就贴回去。
pub fn reveal(app: &AppHandle, id: i64) {
    let Some(o) = shut() else { return };
    log::info!("表情包 存进库后到面板 id={id}");
    crate::meme_window::hide(app, None);
    to_panel(app, o.back, crate::events::MEME_REVEAL, id);
}

/// 发给面板的 `meme-back`：留了草稿没有；贴过去没成、退成只复制时是原因（同面板粘贴的 `CopyResult.reason`）
#[derive(Clone, Serialize)]
struct BackPayload {
    draft: bool,
    reason: Option<&'static str>,
}

/// 返回面板：关掉窗口，唤起面板，不切分类、不改选中。`draft`：这次关窗口留了草稿，面板提示一句
pub fn back(app: &AppHandle, draft: bool) {
    let Some(o) = shut() else { return };
    log::info!("表情包 返回面板{}", if draft { "（留了草稿）" } else { "" });
    crate::meme_window::hide(app, None);
    let payload = BackPayload {
        draft,
        reason: None,
    };
    to_panel(app, o.back, crate::events::MEME_BACK, payload);
}

/// 贴到原来的应用（剪切板已写好）：关掉窗口、把前台交还给打开前的窗口，替用户按一次粘贴。
/// 做不了模拟粘贴、粘贴失败时退成只复制，唤起面板给出面板粘贴同样的降级提示。返回贴过去没有；窗口已经关了时不粘贴
pub fn paste_back(app: &AppHandle, id: i64) -> bool {
    use crate::commands::records::{CopyResult, paste_blocking, paste_or_degrade};
    let Some(o) = shut() else { return false };
    crate::meme_window::hide(app, o.back);
    let r = paste_or_degrade(
        xpaste_platform::can_paste(o.back),
        o.back,
        &format!("表情包 id={id}"),
        || {},
        || paste_blocking(app, o.back),
        || {},
    );
    let CopyResult::CopiedOnly { reason } = r else {
        return true;
    };
    let payload = BackPayload {
        draft: false,
        reason: Some(reason),
    };
    to_panel(app, o.back, crate::events::MEME_BACK, payload);
    false
}

/// 制作窗口关掉之后唤起面板：粘贴目标还是打开制作窗口前的那个，再把 `event` 发给面板
fn to_panel<S: Serialize + Clone + Send + 'static>(
    app: &AppHandle,
    back: Option<isize>,
    event: &'static str,
    payload: S,
) {
    app.state::<AppState>()
        .previous_window
        .store(back.unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
    crate::panel::reshow(app);
    // 排在面板显示之后发：panel-shown 会把搜索清空、重画列表
    crate::panel::on_main(app, move |app| {
        crate::events::emit_to(app, "panel", event, payload)
    });
}

/// 窗口记成关闭，返回打开时记下的来处；本来就没开时返回 None
fn shut() -> Option<Origin> {
    let origin = {
        let mut rt = rt();
        if !rt.open {
            return None;
        }
        rt.open = false;
        rt.refs.clear();
        std::mem::replace(&mut rt.origin, NOWHERE)
    };
    Some(origin)
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
    let _ = open(app, &[], Entry::Other);
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
        if matches!(s.kind, Kind::Image | Kind::File)
            && let Ok((refs, _)) = from_selection(vec![media(&store, s.id, s.kind)])
        {
            out.extend(refs);
        }
    }
    out.truncate(REFS_MAX);
    Ok(out)
}

const PASTE_PAUSED: &str = "暂停记录时加不了剪切板里的图";
const PASTE_MISSING: &str = "这张图没进历史（暂停记录、被忽略的应用或敏感内容时不收），加不了";
const DROP_PAUSED: &str = "暂停记录时加不了拖进来的图";
/// 拖进来的有图片文件，却一张也没加上（过大、空的被采集规则跳过）
const DROP_NONE: &str = "这几张图加不进来";

/// 这条记录能当参考图的部分（同从面板带进来时的拆法）；记录没了、不是图片时为空
fn record_refs(state: &AppState, id: i64) -> Vec<RefKey> {
    from_selection(gather(state, &[id])).map_or_else(|_| Vec::new(), |(refs, _)| refs)
}

/// 窗口里按 ⌘V：只引用采集管道已经收下的那条记录，自己不入库。剪切板里是图片（或图片文件）时返回它的参考图，
/// 文件记录按文件拆开；是文字等用不上的、是 xPaste 自己写进去又对应不到记录的，为空。
/// 暂停记录时、这次没进历史（没收、监听还没处理到、启动前复制的）时返回给用户看的原因
pub fn paste_image(app: &AppHandle) -> Result<Vec<RefKey>, String> {
    use std::sync::atomic::Ordering::SeqCst;
    let state = app.state::<AppState>();
    let r = paste_refs(
        xpaste_platform::change_count(),
        crate::clipboard::last_seen(),
        state.last_write_seq.load(SeqCst),
        state.paused.load(SeqCst),
        |id| record_refs(&state, id),
    );
    match &r {
        Ok(refs) => log::info!("表情包 剪切板里的图加成参考图 {} 张", refs.len()),
        Err(_) => log::info!("表情包 剪切板里的这次没进历史，不加"),
    }
    r.map_err(Into::into)
}

/// ⌘V 的判断：`now` 是剪切板此刻的序号，`last` 是采集管道最后处理的那次（序号、收下的记录 id），
/// `own` 是 xPaste 自己最后写入的序号。只有此刻就是管道收下的那次时才取那条记录的参考图（`refs_of`）
fn paste_refs(
    now: u64,
    last: Option<(u64, Option<i64>)>,
    own: u64,
    paused: bool,
    refs_of: impl FnOnce(i64) -> Vec<RefKey>,
) -> Result<Vec<RefKey>, &'static str> {
    if paused {
        return Err(PASTE_PAUSED);
    }
    match last {
        Some((seq, Some(id))) if seq == now => Ok(refs_of(id)),
        // 自己写进去的、又没记下是哪条记录（比如刚复制的表情包）：当作没有图
        _ if now == own => Ok(Vec::new()),
        _ => Err(PASTE_MISSING),
    }
}

/// 拖进窗口的文件：只收真实存在的图片文件，拼成一条记录按采集规则入库（全是图片文件时存成图片记录），
/// 按文件拆成参考图（同从面板带进来时的拆法）。暂停记录时不入库，返回原因
pub fn add_files(app: &AppHandle, paths: Vec<PathBuf>) -> Result<Vec<RefKey>, String> {
    let state = app.state::<AppState>();
    if state.paused.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(DROP_PAUSED.into());
    }
    let n = paths.len();
    let refs = files_refs(&state, paths, |s| {
        crate::clipboard::record(app, s).map(|(id, _)| id)
    })?;
    log::info!("表情包 拖进来 {n} 个文件，加成参考图 {} 张", refs.len());
    Ok(refs)
}

/// 图片文件拼成一个快照交给 `record` 入库（返回记录 id），再按那条记录拆成参考图；没有图片文件时为空，
/// 有图片文件却一张也没加上（没入库）时返回提示
fn files_refs(
    state: &AppState,
    paths: Vec<PathBuf>,
    record: impl FnOnce(xpaste_core::Snapshot) -> Option<i64>,
) -> Result<Vec<RefKey>, &'static str> {
    let files: Vec<PathBuf> = paths
        .into_iter()
        .filter(|p| p.is_file() && xpaste_core::detect::is_image_file(p))
        .collect();
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let snap = xpaste_core::Snapshot {
        flavors: file_flavors(&files),
        file_paths: files,
        ..Default::default()
    };
    let refs = record(snap).map_or_else(Vec::new, |id| record_refs(state, id));
    if refs.is_empty() {
        return Err(DROP_NONE);
    }
    Ok(refs)
}

/// 几个文件的剪切板格式（同复制文件时系统给的），记录粘贴、复制时原样写回：Windows 一个 `CF_HDROP`
#[cfg(windows)]
fn file_flavors(paths: &[PathBuf]) -> Vec<xpaste_core::Flavor> {
    vec![xpaste_core::Flavor {
        item: 0,
        format: "CF_HDROP".into(),
        data: xpaste_platform::winfmt::dropfiles(paths),
    }]
}

/// 几个文件的剪切板格式：macOS 每个文件一项 `public.file-url`
#[cfg(not(windows))]
fn file_flavors(paths: &[PathBuf]) -> Vec<xpaste_core::Flavor> {
    paths
        .iter()
        .enumerate()
        .flat_map(|(i, p)| {
            xpaste_platform::file_flavors(p)
                .into_iter()
                .map(move |f| xpaste_core::Flavor {
                    item: i as u32,
                    ..f
                })
        })
        .collect()
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
    fn origin_by_entry() {
        let fg = || (Some(7), Some("微信".to_string()));
        let o = arrive(None, Entry::Panel, fg);
        assert!(o.from_panel);
        assert_eq!(o.target.as_deref(), Some("微信"));
        let o = arrive(None, Entry::Other, fg);
        assert!(!o.from_panel);
        assert_eq!((o.back, o.target.as_deref()), (Some(7), Some("微信")));
        // 托盘打开没有粘贴目标，也不取前台
        let o = arrive(None, Entry::Tray, || panic!("托盘打开不取前台"));
        assert_eq!((o.back, o.target, o.tray), (None, None, true));
        // 取不到名字时没有目标（页面按没有目标处理）
        assert_eq!(arrive(None, Entry::Other, || (Some(7), None)).target, None);
    }

    #[test]
    fn retarget_follows_front_on_mac_only() {
        let wx = Origin {
            back: None,
            target: Some("微信".into()),
            from_panel: false,
            tray: false,
        };
        // macOS：换成此刻最前面的应用；前台是 xPaste 自己时没有目标，再切回别的应用又有
        let mut o = wx.clone();
        assert_eq!(
            retarget(&mut o, true, || Some("备忘录".into())).as_deref(),
            Some("备忘录")
        );
        assert_eq!(o.target.as_deref(), Some("备忘录"));
        assert_eq!(retarget(&mut o, true, || None), None);
        assert_eq!(
            retarget(&mut o, true, || Some("微信".into())).as_deref(),
            Some("微信")
        );
        // 从托盘打开的一直没有目标，也不取前台
        let mut t = arrive(None, Entry::Tray, || panic!("托盘打开不取前台"));
        assert_eq!(retarget(&mut t, true, || panic!("托盘打开不取前台")), None);
        // Windows：贴回打开时记下的窗口，名字不改
        let mut w = Origin {
            back: Some(7),
            ..wx
        };
        assert_eq!(
            retarget(&mut w, false, || panic!("不取前台")).as_deref(),
            Some("微信")
        );
        assert_eq!(w.back, Some(7));
    }

    #[test]
    fn origin_kept_while_open() {
        let had = Origin {
            back: Some(7),
            target: Some("微信".into()),
            from_panel: false,
            tray: false,
        };
        let other = || (Some(9), Some("备忘录".to_string()));
        // 已开着再打开：目标不变；快捷键、托盘不改“从面板来”
        assert_eq!(arrive(Some(had.clone()), Entry::Other, other), had);
        assert_eq!(arrive(Some(had.clone()), Entry::Tray, other), had);
        // 从面板带图进来：变成从面板来的，目标还是原来的
        let o = arrive(Some(had.clone()), Entry::Panel, other);
        assert!(o.from_panel);
        assert_eq!((o.back, o.target), (had.back, had.target.clone()));
        let from_panel = Origin {
            from_panel: true,
            ..had
        };
        assert!(arrive(Some(from_panel), Entry::Other, other).from_panel);
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

    #[test]
    fn dropped_files_only_images() {
        let dir = std::env::temp_dir().join(format!("xpaste-meme-drop-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("夹子.png")).unwrap();
        for f in ["a.png", "b.pdf"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let state = AppState::for_test();
        let save = |snap: xpaste_core::Snapshot| {
            let kind = xpaste_core::detect::kind(&snap);
            let rec = xpaste_core::capture::build_record(snap, kind, 0);
            match state.store().insert_or_merge(&rec, 0, 500).unwrap().0 {
                xpaste_core::store::Upsert::Added(id) | xpaste_core::store::Upsert::Merged(id) => {
                    Some(id)
                }
            }
        };
        // PDF、不存在的、名字像图片的文件夹都不收
        let paths = ["a.png", "b.pdf", "没有.png", "夹子.png"].map(|f| dir.join(f));
        let refs = files_refs(&state, paths.to_vec(), save).unwrap();
        assert_eq!(refs.len(), 1);
        // 和从面板带同一条记录进来时的键一样（同一张图不会有两个键）
        assert_eq!(refs, record_refs(&state, refs[0].id));
        assert_eq!(refs[0].file, Some(0));
        // 一张图片都没有：不入库、不提示；有图片却一张也没加上（采集规则跳过了过大、空的）：提示
        assert_eq!(
            files_refs(&state, vec![dir.join("b.pdf")], |_| panic!("不该入库")),
            Ok(vec![])
        );
        assert_eq!(
            files_refs(&state, vec![dir.join("a.png")], |_| None),
            Err(DROP_NONE)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paste_only_what_history_took() {
        let pic = |id| vec![r(id, None)];
        // 此刻就是管道收下的那次：用那条记录
        assert_eq!(
            paste_refs(7, Some((7, Some(3))), u64::MAX, false, pic),
            Ok(pic(3))
        );
        // 收下的是文字：没有图，不提示
        assert_eq!(
            paste_refs(7, Some((7, Some(3))), u64::MAX, false, |_| vec![]),
            Ok(vec![])
        );
        // 那次没收（暂停、忽略的应用、敏感）、管道还没处理到、启动前复制的：不入库，说原因
        assert_eq!(
            paste_refs(7, Some((7, None)), u64::MAX, false, pic),
            Err(PASTE_MISSING)
        );
        assert_eq!(
            paste_refs(8, Some((7, Some(3))), u64::MAX, false, pic),
            Err(PASTE_MISSING)
        );
        assert_eq!(
            paste_refs(8, None, u64::MAX, false, pic),
            Err(PASTE_MISSING)
        );
        // 暂停记录时
        assert_eq!(
            paste_refs(7, Some((7, Some(3))), u64::MAX, true, pic),
            Err(PASTE_PAUSED)
        );
        // xPaste 自己写的、对应不到记录：当作没有图
        assert_eq!(paste_refs(9, Some((7, Some(3))), 9, false, pic), Ok(vec![]));
    }

    #[test]
    fn no_target_when_xpaste_is_in_front() {
        let wx = || {
            Some(xpaste_core::AppInfo {
                id: "com.tencent.xinWeChat".into(),
                name: "微信".into(),
            })
        };
        assert_eq!(target_name(wx(), false).as_deref(), Some("微信"));
        assert_eq!(target_name(wx(), true), None);
        assert_eq!(target_name(None, false), None);
    }
}
