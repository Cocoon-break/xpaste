//! 搅拌机运行时：开着时每次记进历史的复制都掉进杯子，之后按配方让 AI 合成一条。只在带 Pro 的构建里有东西可放，杯子不落盘。
//!
//! 锁顺序同传送带：`RT` 在最外层，持有它时可以短暂去拿 `settings`、`line`，反过来不行；持有 `RT` 时不发事件、不读文件，
//! 要发的事件攒进 `Fx`，放锁后由 `apply` 发。传送带的 `RT` 和这里的 `RT` 从不同时持有。
//! 例外：结果写剪切板在 `RT` 里做（同传送带写“下一件”），采集回调要先拿 `RT`，等这次的序号记好才判断是不是结果。

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::Serialize;
use tauri::{AppHandle, Manager};
use xpaste_core::Kind;
use xpaste_core::model::Recipe;
use xpaste_core::textfile::{TEXT_FILE_MAX_BYTES, decode_text, looks_binary};
use xpaste_pro::{
    Body, CUP_MAX, Cup, CupError, Ingredient, IngredientKind, MAX_CHARS, Material, Pro,
    RECIPES_MAX, Taken,
};

use crate::collect::{self, Taker, Toggle};
use crate::error::AppError;
use crate::events::{MIXER_CHANGED, MIXER_DELTA, MIXER_PUT_SELECTED, MIXER_TOAST, emit_to};
use crate::panel::Rect;
use crate::state::AppState;
use crate::translate::{Coalescer, Fail};

pub const LABEL: &str = "mixer";

/// 搅拌机是否编入此构建（与传送带同一个条件）。
pub fn enabled() -> bool {
    crate::line::enabled()
}

/// 配方列表补齐自带配方、去掉坏项。读入设置、保存设置和测试用的状态都走这里，内存里的设置总是整理过的。
pub fn normalize_settings(settings: &mut xpaste_core::Settings) {
    if enabled() {
        settings.mixer_recipes =
            xpaste_pro::normalize_recipes(std::mem::take(&mut settings.mixer_recipes));
    }
}

/// 这次入库的复制是不是搅拌机自己写的结果：是就不放进杯子。xPaste 别的普通写入（截图复制等）照常放。
pub fn takes_copy(seq: u64, result_seq: u64) -> bool {
    seq != result_seq
}

/// 能不能打开、往杯里放新东西：试用中、已授权时能；试用结束后不行（开着的杯子还能把剩下的搅完）。
pub fn accepts_new(pro: &Pro) -> bool {
    matches!(pro, Pro::Trial { .. } | Pro::Licensed { .. })
}

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Idle,
    Blending,
    Ready,
    Failed,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Cause {
    Put,
    Remove,
    Clear,
    Blend,
    Stop,
    Ready,
    Fail,
    Paste,
    Other,
}

struct Runtime {
    /// 设置里的总开关
    on: bool,
    open: bool,
    /// 收成了角标：内容和进行中的事都照旧
    mini: bool,
    /// 打开时取的戳：和传送带同开时，新复制归后打开的那个
    opened: u64,
    cup: Cup,
    phase: Phase,
    /// 每次开搅、停下、关掉、贴出加一：请求结束时它变了就丢掉结果，贴出后的计时也据此作废
    round: u64,
    /// 进行中的请求，停下、重搅时中止
    task: Option<tauri::async_runtime::JoinHandle<()>>,
    /// 这次结果用的配方名（None 为只按补一句）和补的那句：重搅、换配方、重试时沿用
    recipe: Option<String>,
    extra: String,
    /// 贴出前杯里的原料、用的配方和提示出现的时刻：“再搅一次”用
    again: Option<(Vec<Ingredient>, Option<String>, Instant)>,
    /// 结果在剪切板里时监听粘贴键
    watch: Option<xpaste_platform::PasteKeyWatch>,
    installing: bool,
    /// 结果的标题、正文与失败原因
    title: Option<String>,
    text: String,
    reason: Option<String>,
    /// 写结果时的剪切板序号；没写过时为 `u64::MAX`
    result_seq: u64,
    /// 剪切板里此刻是不是结果
    clip_is_result: bool,
    taken: Option<Taken>,
    cleared: Option<Vec<Ingredient>>,
    warned_expired: bool,
}

static RT: LazyLock<Mutex<Runtime>> = LazyLock::new(|| {
    Mutex::new(Runtime {
        on: false,
        open: false,
        mini: false,
        opened: 0,
        cup: Cup::new(),
        phase: Phase::Idle,
        round: 0,
        task: None,
        recipe: None,
        extra: String::new(),
        again: None,
        watch: None,
        installing: false,
        title: None,
        text: String::new(),
        reason: None,
        result_seq: u64::MAX,
        clip_is_result: false,
        taken: None,
        cleared: None,
        warned_expired: false,
    })
});

fn rt() -> MutexGuard<'static, Runtime> {
    RT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 页面报来的可点区域（窗口内逻辑像素），点击穿透用
pub(crate) static HITS: Mutex<Vec<Rect>> = Mutex::new(Vec::new());

/// `mixer-changed` 的载荷，也是 `mixer_state` 的返回值
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MixerState {
    open: bool,
    mini: bool,
    phase: Phase,
    /// 当前这一次的序号：`mixer-delta` 带的 gen 和它不同时是旧请求的，丢掉
    #[serde(rename = "gen")]
    round: u64,
    items: Vec<Ingredient>,
    /// 打开的配方名，按顺序
    keys: Vec<String>,
    /// 当前或上一次用的配方名；只按补一句时为 None
    recipe: Option<String>,
    title: Option<String>,
    model: String,
    text: String,
    reason: Option<String>,
    clip_is_result: bool,
    pro: Pro,
    cause: Cause,
    cause_id: Option<u64>,
    shake: bool,
}

/// `mixer-toast` 的载荷：`actions` 是提示条上的按钮，`name` 是相关的配方名。不派生 `Debug`：名字是用户写的。
#[derive(Clone, Serialize)]
pub struct Toast {
    text: String,
    actions: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

fn toast(text: impl Into<String>, actions: &[&'static str]) -> Toast {
    Toast {
        text: text.into(),
        actions: actions.to_vec(),
        name: None,
    }
}

fn expired_toast() -> Toast {
    toast("试用已结束，搅拌机要 xPaste Pro", &["license", "buy"])
}

fn expired_open_toast() -> Toast {
    toast("试用已结束，杯子里只能搅完剩下的", &[])
}

fn blending_toast() -> Toast {
    toast("正在搅，这一样没放进去", &[])
}

/// 放不进杯子的原因
#[derive(Debug, PartialEq)]
enum Refuse {
    Video,
    /// 文件夹、PDF、Office 文档、二进制文件、解不出来的文本
    Unsupported,
    /// 超过字数上限（`Some(字数)`），或文件太大没读（`None`）
    TooLong(Option<usize>),
    Missing,
    Full,
}

impl Refuse {
    fn text(&self) -> String {
        match self {
            Refuse::Video => "视频放不进搅拌机".into(),
            Refuse::Unsupported => "这种文件放不进搅拌机，可以复制里面的文字再放".into(),
            Refuse::TooLong(Some(n)) => {
                format!("文件太长（{n} 字），最多 {MAX_CHARS} 字，可以复制其中一段再放")
            }
            Refuse::TooLong(None) => {
                format!("文件太长，最多 {MAX_CHARS} 字，可以复制其中一段再放")
            }
            Refuse::Missing => "原文件已移动或删除".into(),
            Refuse::Full => format!("杯子满了（{CUP_MAX} 样），先搅一搅"),
        }
    }
}

/// 一样原料（种类、标题、内容），或放不进去的原因
type Piece = Result<(IngredientKind, String, Body), Refuse>;

/// 持有 `RT` 期间攒下的、要放锁后再做的事
#[derive(Default)]
struct Fx {
    changed: Option<(Cause, Option<u64>)>,
    shake: bool,
    toasts: Vec<Toast>,
    /// 显示窗口；收起后等页面放手再藏；立刻藏（关掉总开关）
    show: bool,
    linger: bool,
    hide: bool,
}

impl Fx {
    fn changed(&mut self, cause: Cause, id: Option<u64>) {
        self.changed = Some((cause, id));
    }
}

fn pro(app: &AppHandle) -> Pro {
    app.state::<AppState>()
        .line()
        .status(crate::clock::now_ms() / 1000)
}

fn snapshot(app: &AppHandle, cause: Cause, cause_id: Option<u64>, shake: bool) -> MixerState {
    let pro = pro(app);
    let (keys, model) = {
        let state = app.state::<AppState>();
        let s = state.settings();
        let keys = s
            .mixer_recipes
            .iter()
            .filter(|r| r.show)
            .map(|r| r.name.clone())
            .collect();
        (keys, s.translate_model.clone())
    };
    let rt = rt();
    MixerState {
        open: rt.open,
        mini: rt.mini,
        phase: rt.phase,
        round: rt.round,
        items: rt.cup.items().to_vec(),
        keys,
        recipe: rt.recipe.clone(),
        title: rt.title.clone(),
        model,
        text: rt.text.clone(),
        reason: rt.reason.clone(),
        clip_is_result: rt.clip_is_result,
        pro,
        cause,
        cause_id,
        shake,
    }
}

/// 放锁后做 `fx` 里攒下的事：发事件、显示或收起窗口。不要在持有 `RT` 时调用。
fn apply(app: &AppHandle, mut fx: Fx) {
    // 收着时的提示条（试用结束等）：先把窗口显示出来，提示消失后页面放手
    if !fx.toasts.is_empty() && !fx.hide && closed() {
        fx.show = true;
        fx.linger = true;
    }
    for t in fx.toasts {
        emit_to(app, LABEL, MIXER_TOAST, t);
    }
    if let Some((cause, id)) = fx.changed {
        emit_to(
            app,
            LABEL,
            MIXER_CHANGED,
            snapshot(app, cause, id, fx.shake),
        );
    }
    if fx.show {
        crate::mixer_window::show(app);
    }
    if fx.hide {
        crate::mixer_window::hide(app);
    } else if fx.linger {
        crate::mixer_window::linger(app);
    }
    sync_watch(app);
}

/// 在 `RT` 里改一下杯子，放锁后发攒下的事件。
fn with(app: &AppHandle, f: impl FnOnce(&Pro, &mut Runtime, &mut Fx)) {
    let pro = pro(app);
    let mut fx = Fx::default();
    f(&pro, &mut rt(), &mut fx);
    apply(app, fx);
}

/// 开着时返回打开时的戳
pub fn open_stamp() -> Option<u64> {
    let rt = rt();
    rt.open.then_some(rt.opened)
}

/// 收着：窗口该藏起来了
pub fn closed() -> bool {
    !rt().open
}

/// 启动时调用（调用方先看过 `enabled`）：预建隐藏的窗口。
pub fn start(app: &AppHandle) {
    crate::mixer_window::prewarm(app);
    log::info!("搅拌机就绪");
}

/// 启动和保存设置时调用：记下总开关。关掉时直接倒掉、收起，不弹撤销；开着时把新的配方、模型发给页面。
pub fn configure(app: &AppHandle, on: bool) {
    with(app, |_, rt, fx| {
        rt.on = on && enabled();
        if !rt.on {
            rt.again = None;
            rt.cleared = None;
            if rt.open {
                close(rt, fx, false);
            }
            fx.hide = true;
        } else if rt.open {
            fx.changed(Cause::Other, None);
        }
    });
}

/// 搅拌机快捷键按下（主线程）：面板在屏上且拿着键盘时让面板把选中的放进来；否则开关搅拌机。
pub fn hotkey_pressed(app: &AppHandle) {
    if crate::panel::visible(app) && crate::panel::holds_key(app) {
        emit_to(app, "panel", MIXER_PUT_SELECTED, ());
        return;
    }
    // 不占着主线程拿锁
    let app = app.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("mixer-hotkey".into())
        .spawn(move || toggle(&app))
    {
        log::warn!("搅拌机线程 mixer-hotkey 启动失败：{e}");
    }
}

fn toggle(app: &AppHandle) {
    with(app, |pro, rt, fx| {
        if !rt.on {
            return;
        }
        match collect::toggle(rt.open, rt.mini, !rt.cup.items().is_empty()) {
            Toggle::Open if accepts_new(pro) => {
                open(rt, fx);
                fx.changed(Cause::Other, None);
            }
            Toggle::Open => fx.toasts.push(expired_toast()),
            Toggle::Expand => set_mini(rt, fx, false),
            Toggle::Mini => set_mini(rt, fx, true),
            Toggle::Close => close(rt, fx, true),
        }
    });
}

fn set_mini(rt: &mut Runtime, fx: &mut Fx, on: bool) {
    if rt.mini == on {
        return;
    }
    rt.mini = on;
    fx.changed(Cause::Other, None);
    log::info!("搅拌机 {}", if on { "收成角标" } else { "展开" });
}

/// 收成角标或展开（只在开着时生效）
pub fn mini(app: &AppHandle, on: bool) {
    with(app, |_, rt, fx| {
        if rt.open {
            set_mini(rt, fx, on);
        }
    });
}

#[derive(Debug, PartialEq)]
enum AfterPaste {
    Close,
    ShakeThenClose,
}

/// 贴出结果之后：角标时直接消失，展开时抖一下、提示，再收起
fn after_paste(mini: bool) -> AfterPaste {
    if mini {
        AfterPaste::Close
    } else {
        AfterPaste::ShakeThenClose
    }
}

fn open(rt: &mut Runtime, fx: &mut Fx) {
    rt.open = true;
    rt.mini = false;
    fx.show = true;
    rt.opened = collect::stamp();
    rt.taken = None;
    rt.cleared = None;
    rt.warned_expired = false;
    log::info!("搅拌机 打开");
}

/// 收起；杯里还有东西时倒掉，`undo` 时提示可以撤销。
fn close(rt: &mut Runtime, fx: &mut Fx, undo: bool) {
    let n = rt.cup.items().len();
    let poured = rt.cup.clear();
    rt.cleared = (undo && n > 0).then_some(poured);
    if undo && n > 0 {
        fx.toasts
            .push(toast(format!("已倒掉 {n} 样"), &["undoClear"]));
    }
    rt.open = false;
    rt.mini = false;
    fx.linger = true;
    cancel(rt);
    reset_result(rt);
    rt.taken = None;
    fx.changed(Cause::Clear, None);
    log::info!("搅拌机 收起 倒掉 {n} 样");
}

/// 采集回调的第一件事（每次剪切板变化）：结果在剪切板里时，变成了别的内容就记下“不再是结果”。
pub fn on_change(app: &AppHandle, seq: u64) {
    {
        let mut rt = rt();
        if !rt.clip_is_result || seq == rt.result_seq {
            return;
        }
        rt.clip_is_result = false;
    }
    emit_to(
        app,
        LABEL,
        MIXER_CHANGED,
        snapshot(app, Cause::Other, None, false),
    );
    sync_watch(app);
}

/// 一次复制记进了历史（新增或合并）之后：搅拌机开着、新复制归它时放进杯子。
pub fn on_recorded(app: &AppHandle, id: i64, kind: Kind, seq: u64) {
    // 两边的锁各拿各的、一次只拿一个
    if collect::taker(crate::belt::open_stamp(), open_stamp()) != Some(Taker::Mixer) {
        return;
    }
    if !takes_copy(seq, rt().result_seq) {
        return;
    }
    // 读库、读文件都不持锁
    let pieces = gather(&app.state::<AppState>(), &[id]);
    with(app, |pro, rt, fx| {
        if !rt.open || !takes_copy(seq, rt.result_seq) {
            return;
        }
        if !accepts_new(pro) {
            if !std::mem::replace(&mut rt.warned_expired, true) {
                fx.toasts.push(expired_open_toast());
            }
            return;
        }
        if rt.phase == Phase::Blending {
            fx.toasts.push(blending_toast());
            return;
        }
        let (n, last) = place(rt, fx, pieces);
        log::info!("搅拌机 放进 {n} 样 id={id} kind={kind:?}");
        fx.changed(Cause::Put, last);
    });
}

/// 面板里放进几条记录（按给的顺序排在最后），搅拌机没开就顺便打开。
pub fn put(app: &AppHandle, ids: &[i64]) {
    if !rt().on {
        return;
    }
    let pieces = gather(&app.state::<AppState>(), ids);
    with(app, |pro, rt, fx| {
        if !rt.on {
            return;
        }
        if !accepts_new(pro) {
            fx.toasts.push(if rt.open {
                expired_open_toast()
            } else {
                expired_toast()
            });
            return;
        }
        if rt.phase == Phase::Blending {
            fx.toasts.push(blending_toast());
            return;
        }
        if !rt.open {
            open(rt, fx);
        }
        let (n, last) = place(rt, fx, pieces);
        log::info!("搅拌机 放进 {n} 样");
        fx.changed(Cause::Put, last);
    });
}

/// 把读好的原料依次放进杯子，返回放进去的样数和最后一样的 id。有放不进去的提示一次：
/// 只有一样时说它自己的原因，几样时说“放进去 N 样，M 样放不进去”；满了时杯子抖一下。
fn place(rt: &mut Runtime, fx: &mut Fx, pieces: Vec<(i64, Piece)>) -> (usize, Option<u64>) {
    let total = pieces.len();
    let (mut n, mut last, mut refused) = (0, None, None);
    for (record_id, piece) in pieces {
        let r = piece.and_then(|(kind, title, body)| {
            match rt.cup.push(kind, title, Some(record_id), body) {
                Ok(id) => Ok(id),
                Err(CupError::Full) => Err(Refuse::Full),
                // 只有不含 Pro 的空实现会这样，那时搅拌机不会打开
                Err(CupError::NotFound) => Err(Refuse::Unsupported),
            }
        });
        match r {
            Ok(id) => {
                n += 1;
                last = Some(id);
            }
            Err(e) => {
                fx.shake |= e == Refuse::Full;
                refused = Some(e);
            }
        }
    }
    if n > 0 {
        rt.cleared = None;
    }
    if let Some(e) = refused {
        fx.toasts.push(toast(
            if total == 1 {
                e.text()
            } else {
                format!("放进去 {n} 样，{} 样放不进去", total - n)
            },
            &[],
        ));
    }
    (n, last)
}

/// 读出几条记录要放进杯子的原料（一条文件记录按文件拆成几样）。已删的记录跳过。不要在持有 `RT` 时调用。
fn gather(state: &AppState, ids: &[i64]) -> Vec<(i64, Piece)> {
    let mut out = Vec::new();
    for &id in ids {
        match read_record(state, id) {
            Ok(Some(pieces)) => out.extend(pieces.into_iter().map(|p| (id, p))),
            Ok(None) => log::info!("搅拌机 记录已删除，不放进去 id={id}"),
            Err(e) => log::warn!("搅拌机 读取记录 {id} 失败：{e}"),
        }
    }
    out
}

fn read_record(state: &AppState, id: i64) -> Result<Option<Vec<Piece>>, xpaste_core::CoreError> {
    // 只在读库时持 store 锁，读文件时放掉
    let (kind, title, text, paths) = {
        let store = state.store();
        let Some(s) = store.live_summary(id)? else {
            return Ok(None);
        };
        let (text, paths): (Option<String>, Vec<PathBuf>) = match s.kind {
            Kind::Text | Kind::Code | Kind::Link => (store.text(id)?, Vec::new()),
            Kind::File => (
                None,
                store
                    .detail(id)?
                    .map(|d| d.files.into_iter().map(|f| PathBuf::from(f.path)).collect())
                    .unwrap_or_default(),
            ),
            Kind::Image | Kind::Video => (None, Vec::new()),
        };
        (s.kind, s.title, text, paths)
    };
    let text_kind = match kind {
        Kind::Link => IngredientKind::Link,
        Kind::Code => IngredientKind::Code,
        _ => IngredientKind::Text,
    };
    Ok(Some(match kind {
        Kind::Text | Kind::Code | Kind::Link => text
            .map(|t| Ok((text_kind, title, Body::Text(t))))
            .into_iter()
            .collect(),
        Kind::Image => vec![Ok((
            IngredientKind::Image,
            title,
            Body::Image {
                record_id: id,
                path: None,
            },
        ))],
        Kind::Video => vec![Err(Refuse::Video)],
        Kind::File => paths.iter().map(|p| read_file(id, p)).collect(),
    }))
}

/// 文件记录里的一个文件：图片文件记路径（开搅时才读），纯文本文件读成文字，其余的不收。
fn read_file(record_id: i64, path: &Path) -> Piece {
    let Ok(meta) = std::fs::metadata(path) else {
        return Err(Refuse::Missing);
    };
    if meta.is_dir() {
        return Err(Refuse::Unsupported);
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if xpaste_core::detect::is_image_file(path) {
        let body = Body::Image {
            record_id,
            path: Some(path.to_path_buf()),
        };
        return Ok((IngredientKind::Image, name, body));
    }
    // PDF 的开头常常没有 NUL，按扩展名先拦下
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
    {
        return Err(Refuse::Unsupported);
    }
    // 先只读开头 8 KB 看是不是二进制：大的压缩包、文档也该按“这种文件”拒，不按“太长”
    let mut head = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(8192).read_to_end(&mut head))
        .map_err(|_| Refuse::Missing)?;
    if looks_binary(&head) {
        return Err(Refuse::Unsupported);
    }
    if meta.len() > TEXT_FILE_MAX_BYTES {
        return Err(Refuse::TooLong(None));
    }
    let bytes = std::fs::read(path).map_err(|_| Refuse::Missing)?;
    let text = decode_text(&bytes).ok_or(Refuse::Unsupported)?;
    let n = text.chars().count();
    if n > MAX_CHARS {
        return Err(Refuse::TooLong(Some(n)));
    }
    Ok((IngredientKind::File, name, Body::Text(text)))
}

// ---- 页面与面板调用的操作（`commands::mixer`） ----

pub fn state_now(app: &AppHandle) -> MixerState {
    snapshot(app, Cause::Other, None, false)
}

pub fn ready(app: &AppHandle) {
    emit_to(app, LABEL, MIXER_CHANGED, state_now(app));
}

/// 窗口藏着时不收：藏起以后才到的旧报告不能把点击区域放回去（显示后页面会重报）
pub fn set_hits(rects: Vec<Rect>) {
    if !crate::mixer_window::shown() {
        return;
    }
    *HITS.lock().unwrap_or_else(PoisonError::into_inner) = rects;
}

/// 页面播完收起、提示条也消失了：仍收着就藏窗口（收起途中又打开了就不藏，`hide` 在主线程上再看一次）。
pub fn release(app: &AppHandle) {
    crate::mixer_window::hide(app);
}

/// 补一句展开时拿键盘、收起时交还
pub fn keyboard(app: &AppHandle, on: bool) {
    crate::mixer_window::keyboard(app, on);
}

pub fn remove(app: &AppHandle, id: u64) {
    with(app, |_, rt, fx| {
        if let Ok(t) = rt.cup.remove(id) {
            rt.taken = Some(t);
            fx.toasts.push(toast("已拿掉", &["undoRemove"]));
            log::info!("搅拌机 拿掉 id={id}");
        }
        fx.changed(Cause::Remove, Some(id));
    });
}

pub fn undo_remove(app: &AppHandle) {
    with(app, |_, rt, fx| {
        if !rt.open {
            return;
        }
        if let Some(t) = rt.taken.take() {
            rt.cup.restore(t);
            log::info!("搅拌机 撤销拿掉");
        }
        fx.changed(Cause::Put, None);
    });
}

pub fn close_now(app: &AppHandle) {
    with(app, |_, rt, fx| {
        if rt.open {
            close(rt, fx, true);
        }
    });
}

/// 撤销倒掉：按原来的顺序放回；已收起时重新打开。
pub fn undo_clear(app: &AppHandle) {
    with(app, |pro, rt, fx| {
        // 总开关关了、试用结束后不再重新打开
        if !rt.on || (!rt.open && !accepts_new(pro)) || !rt.cup.items().is_empty() {
            return;
        }
        let Some(c) = rt.cleared.take() else {
            return;
        };
        if !rt.open {
            open(rt, fx);
        }
        rt.cup.refill(c);
        log::info!("搅拌机 撤销倒掉 {} 样", rt.cup.items().len());
        fx.changed(Cause::Put, None);
    });
}

// ---- 搅：请求、停下、结果、贴出 ----

/// 补一句最多这么多字
const EXTRA_MAX: usize = 200;
/// 图片长边上限；编成 PNG 超过 `IMAGE_MAX_BYTES` 时改用 JPEG
pub const IMAGE_EDGE: u32 = 2048;
pub const IMAGE_MAX_BYTES: usize = 4_194_304;
const JPEG_QUALITY: u8 = 85;
/// “再搅一次”在提示出现后多久内有效
const AGAIN_MS: u64 = 4000;
/// 贴出后多久抖一下、提示；再过多久收起
const PASTE_SHAKE_MS: u64 = 380;
const PASTE_HIDE_MS: u64 = 700;

#[derive(Debug, PartialEq)]
pub enum ResultAction {
    Drop,
    Ready,
    Partial,
    Fail,
}

/// 请求结束时怎么处理：不是当前这一次（gen 变了）一律丢掉；成功且非空为 Ready；失败时收到过内容为 Partial，否则 Fail。
pub fn result_action(
    current_gen: u64,
    req_gen: u64,
    ok_nonempty: bool,
    received: bool,
) -> ResultAction {
    if current_gen != req_gen {
        ResultAction::Drop
    } else if ok_nonempty {
        ResultAction::Ready
    } else if received {
        ResultAction::Partial
    } else {
        ResultAction::Fail
    }
}

/// 失败原因的文案；杯里有图片时 400 / 415 / 422 多半是模型看不了图。
pub fn reason(f: &Fail, has_images: bool) -> String {
    match f {
        Fail::Http(400 | 415 | 422) if has_images => {
            "这个模型看不了图片，换一个能看图的模型，或拿掉图片再搅".into()
        }
        Fail::Http(s @ (401 | 403 | 404 | 429)) => xpaste_core::translate::http_error(*s),
        Fail::Http(s) => format!("AI 接口出错（HTTP {s}）"),
        Fail::Config => "AI 接口还没配好".into(),
        Fail::Timeout => "AI 接口超时".into(),
        Fail::Connect => "连不上 AI 接口，请检查网络或接口地址".into(),
        Fail::Internal(e) => e.to_string(),
        _ => "AI 接口返回的内容无法识别".into(),
    }
}

/// 长边缩到 `IMAGE_EDGE` 以内，编成 PNG；超过 `IMAGE_MAX_BYTES` 时改成 JPEG（透明处铺白）。返回 (mime, 字节)。
pub fn encode_image(img: image::DynamicImage) -> Result<(&'static str, Vec<u8>), AppError> {
    let bad = |e: image::ImageError| AppError::Invalid(format!("图片编码失败：{e}"));
    let img = if img.width().max(img.height()) > IMAGE_EDGE {
        img.resize(
            IMAGE_EDGE,
            IMAGE_EDGE,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(bad)?;
    if png.len() <= IMAGE_MAX_BYTES {
        return Ok(("image/png", png));
    }
    let rgba = img.to_rgba8();
    let rgb = image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let a = u16::from(a);
        let over = |c: u8| ((u16::from(c) * a + 255 * (255 - a)) / 255) as u8;
        image::Rgb([over(r), over(g), over(b)])
    });
    let mut jpg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, JPEG_QUALITY)
        .encode_image(&rgb)
        .map_err(bad)?;
    Ok(("image/jpeg", jpg))
}

/// `mixer-delta` 的载荷。不派生 `Debug`：带着结果。
#[derive(Clone, Serialize)]
struct MixerDelta {
    #[serde(rename = "gen")]
    round: u64,
    text: String,
}

/// 发请求前从杯里取出的一样：文字类已经是材料，图片要在后台读出来
enum Src {
    Done(Material),
    Image {
        id: u64,
        record_id: i64,
        path: Option<PathBuf>,
    },
}

/// 中止进行中的请求，并让之后到的结果、贴出后的计时作废
fn cancel(rt: &mut Runtime) {
    rt.round += 1;
    if let Some(t) = rt.task.take() {
        t.abort();
    }
}

/// 结果卡片收起（原料不动）
fn reset_result(rt: &mut Runtime) {
    rt.phase = Phase::Idle;
    rt.title = None;
    rt.text.clear();
    rt.reason = None;
    rt.clip_is_result = false;
}

/// 提示一句、杯子抖一下
fn refuse(fx: &mut Fx, t: Toast) {
    fx.toasts.push(t);
    fx.shake = true;
    fx.changed(Cause::Other, None);
}

fn unconfigured_toast() -> Toast {
    toast("AI 接口还没配好", &["settings"])
}

/// 卡片标题：配方名、配方名 + 那句、或“按你写的”
fn title(recipe: Option<&str>, extra: &str) -> String {
    match recipe {
        Some(r) if extra.is_empty() => r.to_string(),
        Some(r) => format!("{r} + “{extra}”"),
        None => format!("按你写的：“{extra}”"),
    }
}

/// 图片原料的原图：原记录还在、文件也在时返回路径（位图记录在 `images/<id>.png`）。不要在持有 `RT` 时调用。
fn image_file(state: &AppState, record_id: i64, path: Option<&Path>) -> Option<PathBuf> {
    let live = matches!(state.store().live_summary(record_id), Ok(Some(_)));
    let p = path.map(Path::to_path_buf).unwrap_or_else(|| {
        state
            .data_dir
            .join("images")
            .join(format!("{record_id}.png"))
    });
    (live && p.exists()).then_some(p)
}

/// 读出、编码图片，拼成请求的材料；返回材料、图片张数、图片字节数，或读不出来的那几样的 id。
/// ponytail: 解不开的图片也按“原记录没了”拿掉；要区分时给它单独一句提示
fn materials(state: &AppState, srcs: Vec<Src>) -> Result<(Vec<Material>, usize, usize), Vec<u64>> {
    let (mut out, mut missing, mut n, mut bytes) = (Vec::new(), Vec::new(), 0, 0);
    for s in srcs {
        let (id, record_id, path) = match s {
            Src::Done(m) => {
                out.push(m);
                continue;
            }
            Src::Image {
                id,
                record_id,
                path,
            } => (id, record_id, path),
        };
        let encoded = image_file(state, record_id, path.as_deref())
            .and_then(|p| {
                image::ImageReader::open(&p)
                    .ok()?
                    .with_guessed_format()
                    .ok()?
                    .decode()
                    .ok()
            })
            .and_then(|img| encode_image(img).ok());
        match encoded {
            Some((mime, data)) => {
                n += 1;
                bytes += data.len();
                let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                out.push(Material::Image {
                    data_url: format!("data:{mime};base64,{b64}"),
                });
            }
            None => missing.push(id),
        }
    }
    if missing.is_empty() {
        Ok((out, n, bytes))
    } else {
        Err(missing)
    }
}

/// 拿掉原记录没了的图片，提示一次
fn drop_missing(rt: &mut Runtime, fx: &mut Fx, ids: &[u64]) {
    for &id in ids {
        let _ = rt.cup.remove(id);
    }
    log::info!("搅拌机 图片原记录没了，拿掉 {} 样", ids.len());
    refuse(fx, toast("原记录已删除，拿掉了一样", &[]));
}

fn find_recipe(app: &AppHandle, name: &str) -> Result<Recipe, AppError> {
    app.state::<AppState>()
        .settings()
        .mixer_recipes
        .iter()
        .find(|r| r.name == name)
        .cloned()
        .ok_or_else(|| AppError::Invalid("没有这个配方".into()))
}

/// 按配方（None 为只按补一句）和补的那句开搅。配方名不在设置里时报错；两样都没有时什么都不做。
pub fn blend(app: &AppHandle, recipe: Option<String>, extra: String) -> Result<(), AppError> {
    let extra: String = extra.trim().chars().take(EXTRA_MAX).collect();
    let recipe = match recipe {
        Some(name) => Some(find_recipe(app, &name)?),
        None if extra.is_empty() => return Ok(()),
        None => None,
    };
    let cfg = crate::translate::cfg(&app.state::<AppState>().settings());
    let unconfigured = cfg.base_url.trim().is_empty() || cfg.model.trim().is_empty();
    // 先查：杯空、没配置、字数超了；顺带取出图片原料，放锁后看原记录还在不在
    let mut images = None;
    with(app, |_, rt, fx| {
        if !rt.open {
            return;
        }
        if rt.cup.items().is_empty() {
            return refuse(fx, toast("杯子是空的，先放点东西进来", &[]));
        }
        if unconfigured {
            return refuse(fx, unconfigured_toast());
        }
        let n = rt.cup.text_chars() + extra.chars().count();
        if n > MAX_CHARS {
            return refuse(
                fx,
                toast(
                    format!("原料太多了（{n} 字），最多 {MAX_CHARS} 字，拿掉几样再搅"),
                    &[],
                ),
            );
        }
        images = Some(
            rt.cup
                .items()
                .iter()
                .filter_map(|i| match &i.body {
                    Body::Image { record_id, path } => Some((i.id, *record_id, path.clone())),
                    Body::Text(_) => None,
                })
                .collect::<Vec<_>>(),
        );
    });
    let Some(images) = images else {
        return Ok(());
    };
    let state = app.state::<AppState>();
    let missing: Vec<u64> = images
        .into_iter()
        .filter(|(_, rid, p)| image_file(&state, *rid, p.as_deref()).is_none())
        .map(|(id, ..)| id)
        .collect();
    with(app, |_, rt, fx| {
        if !rt.open {
            return;
        }
        if !missing.is_empty() {
            return drop_missing(rt, fx, &missing);
        }
        if rt.cup.items().is_empty() {
            return;
        }
        let srcs: Vec<Src> = rt
            .cup
            .items()
            .iter()
            .map(|i| match &i.body {
                Body::Text(t) => Src::Done(Material::Text {
                    label: match i.kind {
                        IngredientKind::Link => "链接".into(),
                        IngredientKind::Code => "代码".into(),
                        IngredientKind::File => format!("文件 {}", i.title),
                        _ => "文字".into(),
                    },
                    text: t.clone(),
                    code: i.kind == IngredientKind::Code,
                }),
                Body::Image { record_id, path } => Src::Image {
                    id: i.id,
                    record_id: *record_id,
                    path: path.clone(),
                },
            })
            .collect();
        let job = Job {
            items: srcs.len(),
            chars: rt.cup.text_chars(),
            key: match &recipe {
                Some(r) => r.builtin.clone().unwrap_or_else(|| "自定义".into()),
                None => "无".into(),
            },
            body_args: (
                cfg.model.trim().to_string(),
                recipe.as_ref().map(|r| r.prompt.clone()),
                extra.clone(),
            ),
            base_url: cfg.base_url.to_string(),
            api_key: cfg.api_key.to_string(),
        };
        cancel(rt);
        reset_result(rt);
        rt.phase = Phase::Blending;
        rt.title = Some(title(recipe.as_ref().map(|r| r.name.as_str()), &extra));
        rt.recipe = recipe.as_ref().map(|r| r.name.clone());
        rt.extra = extra.clone();
        rt.again = None;
        let (a, g) = (app.clone(), rt.round);
        rt.task = Some(tauri::async_runtime::spawn(run(a, g, srcs, job)));
        fx.changed(Cause::Blend, None);
    });
    Ok(())
}

/// 一次请求要的东西（日志只用其中的件数、字数、键名）。不派生 `Debug`：带着提示词、补的那句和 Key。
struct Job {
    items: usize,
    chars: usize,
    /// 自带配方的键名、“自定义”或“无”（只按补一句）
    key: String,
    /// 模型、配方提示词、补的那句
    body_args: (String, Option<String>, String),
    base_url: String,
    api_key: String,
}

async fn run(app: AppHandle, g: u64, srcs: Vec<Src>, job: Job) {
    let started = Instant::now();
    let a = app.clone();
    let mats =
        tauri::async_runtime::spawn_blocking(move || materials(&a.state::<AppState>(), srcs)).await;
    let (mats, images, bytes) = match mats {
        Ok(Ok(m)) => m,
        Ok(Err(missing)) => {
            return with(&app, |_, rt, fx| {
                if rt.round == g {
                    rt.task = None;
                    reset_result(rt);
                    drop_missing(rt, fx, &missing);
                }
            });
        }
        Err(e) => return finish(&app, g, Err(Fail::Internal(e.into())), false, started),
    };
    let (model, prompt, extra) = &job.body_args;
    log::info!(
        "搅拌机 开搅 {} 样 {} 字 图片 {images} 张 {bytes} 字节 配方={} 补一句={}",
        job.items,
        job.chars,
        job.key,
        !extra.is_empty()
    );
    let body = xpaste_pro::chat_body(model, prompt.as_deref(), extra, &mats);
    drop(mats);
    let mut co = Coalescer::default();
    let r =
        crate::translate::chat_stream(&job.base_url, &job.api_key, &body, None, &mut |d: &str| {
            {
                let mut rt = rt();
                if rt.round != g {
                    return false;
                }
                rt.text.push_str(d);
            }
            if let Some(text) = co.push(d, Instant::now()) {
                emit_to(&app, LABEL, MIXER_DELTA, MixerDelta { round: g, text });
            }
            true
        })
        .await;
    // 剩下没发的增量不用补：结束时的 `mixer-changed` 带着全文
    finish(&app, g, r, images > 0, started);
}

/// 请求结束：只有还是当前这一次时才动剪切板和状态。
fn finish(app: &AppHandle, g: u64, r: Result<String, Fail>, has_images: bool, started: Instant) {
    let (out, fail) = match r {
        Ok(t) => (t.trim().to_string(), None),
        Err(f) => (String::new(), Some(f)),
    };
    let state = app.state::<AppState>();
    with(app, |_, rt, fx| {
        let received = !rt.text.trim().is_empty();
        let action = result_action(rt.round, g, !out.is_empty(), received);
        if action == ResultAction::Drop {
            return;
        }
        rt.task = None;
        log::info!(
            "搅拌机 结束 {} 耗时 {} ms 结果 {} 字",
            fail.as_ref().map_or_else(|| "ok".into(), Fail::tag),
            started.elapsed().as_millis(),
            out.chars().count()
        );
        // 成功但结果是空白时按“返回的内容无法识别”
        let fail = fail.unwrap_or(Fail::Bad);
        match action {
            ResultAction::Ready => {
                rt.text = out;
                rt.phase = Phase::Ready;
                write_result(&state, rt);
                fx.changed(Cause::Ready, None);
            }
            ResultAction::Partial => {
                rt.phase = Phase::Failed;
                rt.reason = Some(reason(&fail, has_images));
                fx.changed(Cause::Fail, None);
            }
            _ => {
                reset_result(rt);
                fx.toasts.push(match fail {
                    Fail::Config => unconfigured_toast(),
                    f => toast(format!("没搅成：{}", reason(&f, has_images)), &["retry"]),
                });
                fx.shake = true;
                fx.changed(Cause::Fail, None);
            }
        }
    });
}

/// 把结果按普通写入写进剪切板，记下这次的序号。在 `RT` 里写：采集回调先拿 `RT`，会等序号记好再判断是不是结果。
fn write_result(state: &AppState, rt: &mut Runtime) {
    match crate::clipboard::write_own(state, &[xpaste_core::text_flavor(&rt.text)], "搅拌机结果")
    {
        Ok(seq) => {
            rt.result_seq = seq;
            rt.clip_is_result = true;
        }
        // 写不进去时卡片显示“剪切板换成了…”，可以点“放回结果”再试
        Err(_) => rt.clip_is_result = false,
    }
}

pub fn stop(app: &AppHandle) {
    with(app, |_, rt, fx| {
        if rt.phase != Phase::Blending {
            return;
        }
        cancel(rt);
        reset_result(rt);
        fx.toasts.push(toast("停了，原料还在", &[]));
        fx.changed(Cause::Stop, None);
        log::info!("搅拌机 停下");
    });
}

/// 重搅（None）或换配方（Some）：沿用这次补的那句。失败后的“重试”也走这里。
pub fn reblend(app: &AppHandle, recipe: Option<String>) -> Result<(), AppError> {
    let (cur, extra) = {
        let rt = rt();
        (rt.recipe.clone(), rt.extra.clone())
    };
    blend(app, recipe.or(cur), extra)
}

/// 倒掉结果：卡片收起，原料还在（历史里那条不删）
pub fn dump(app: &AppHandle) {
    with(app, |_, rt, fx| {
        if !matches!(rt.phase, Phase::Ready | Phase::Failed) {
            return;
        }
        reset_result(rt);
        fx.toasts.push(toast("结果倒掉了，原料还在", &[]));
        fx.changed(Cause::Other, None);
        log::info!("搅拌机 倒掉结果");
    });
}

/// 剪切板换成了别的之后，把结果再写回去
pub fn put_back(app: &AppHandle) {
    let state = app.state::<AppState>();
    with(app, |_, rt, fx| {
        if rt.phase != Phase::Ready || rt.clip_is_result {
            return;
        }
        write_result(&state, rt);
        fx.changed(Cause::Other, None);
        log::info!("搅拌机 放回结果");
    });
}

/// 贴出后提示条上的“再搅一次”：原料按原顺序回到杯里，配方记着，不自动开搅。
pub fn again(app: &AppHandle) {
    with(app, |pro, rt, fx| {
        if !rt.on || rt.phase == Phase::Blending {
            return;
        }
        let Some((items, recipe, at)) = rt.again.take() else {
            return;
        };
        if at.elapsed() > Duration::from_millis(AGAIN_MS) {
            return;
        }
        if !accepts_new(pro) {
            fx.toasts.push(expired_toast());
            return;
        }
        // 作废贴出后的收起计时
        cancel(rt);
        if !rt.open {
            open(rt, fx);
        }
        rt.cup.refill(items);
        rt.recipe = recipe;
        rt.extra.clear();
        fx.changed(Cause::Put, None);
        log::info!("搅拌机 再搅一次 {} 样", rt.cup.items().len());
    });
}

/// 把补的那句存成自己的配方，返回名字。满了时提示并报错。
pub fn save_recipe(app: &AppHandle, text: &str) -> Result<String, AppError> {
    let text: String = text
        .trim()
        .replace(['\r', '\n'], " ")
        .chars()
        .take(EXTRA_MAX)
        .collect();
    if text.is_empty() {
        return Err(AppError::Invalid("补一句是空的".into()));
    }
    let state = app.state::<AppState>();
    let mut s = state.settings().clone();
    if s.mixer_recipes.len() >= RECIPES_MAX {
        let t = format!("配方满了（{RECIPES_MAX} 个），先删掉一些");
        emit_to(app, LABEL, MIXER_TOAST, toast(t.clone(), &[]));
        return Err(AppError::Invalid(t));
    }
    let name = xpaste_pro::saved_name(&text, &s.mixer_recipes);
    s.mixer_recipes.push(Recipe {
        name: name.clone(),
        prompt: text,
        show: true,
        builtin: None,
    });
    let on_base = s
        .mixer_recipes
        .iter()
        .filter(|r| r.show)
        .position(|r| r.name == name)
        .is_some_and(|i| i < 3);
    crate::commands::settings::save_settings_inner(&state, s.clone())?;
    emit_to(app, "panel", crate::events::SETTINGS_CHANGED, s);
    log::info!("搅拌机 存成配方");
    let at = if on_base {
        "在底座上"
    } else {
        "在 ⋯ 里"
    };
    emit_to(
        app,
        LABEL,
        MIXER_TOAST,
        Toast {
            text: format!("已存成配方“{name}”，{at}"),
            actions: vec!["rename"],
            name: Some(name.clone()),
        },
    );
    emit_to(app, LABEL, MIXER_CHANGED, state_now(app));
    Ok(name)
}

// ---- 贴出：结果在剪切板里时监听粘贴键 ----

fn watch_wanted(rt: &Runtime) -> bool {
    rt.phase == Phase::Ready && rt.clip_is_result
}

fn spawn(app: &AppHandle, name: &str, f: impl FnOnce(&AppHandle) + Send + 'static) {
    let app = app.clone();
    if let Err(e) = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || f(&app))
    {
        log::warn!("搅拌机线程 {name} 启动失败：{e}");
    }
}

/// 监听跟上结果：在剪切板里时装上，其余时候卸掉。和传送带各用各的一份。
fn sync_watch(app: &AppHandle) {
    let (want, has) = {
        let rt = rt();
        (watch_wanted(&rt), rt.watch.is_some())
    };
    if want == has {
        return;
    }
    if !want {
        // Windows 上丢弃要等钩子线程退出，在别的线程丢，不卡调用方
        let old = rt().watch.take();
        if let Some(old) = old {
            spawn(app, "mixer-unwatch", move |_| drop(old));
            log::info!("搅拌机 停止监听粘贴键");
        }
        return;
    }
    // macOS 必须在主线程装；Windows 装的时候要等钩子线程就绪，在别的线程装
    #[cfg(target_os = "macos")]
    crate::panel::on_main(app, install_watch);
    #[cfg(not(target_os = "macos"))]
    spawn(app, "mixer-watch", install_watch);
}

fn install_watch(app: &AppHandle) {
    {
        let mut rt = rt();
        if !watch_wanted(&rt) || rt.watch.is_some() || rt.installing {
            return;
        }
        rt.installing = true;
    }
    let a = app.clone();
    let watch = xpaste_platform::watch_paste_key(move |seq| {
        let at_press = seq;
        spawn(&a, "mixer-paste", move |app| pasted(app, at_press));
    });
    let stale = {
        let mut rt = rt();
        rt.installing = false;
        match watch {
            Some(w) if watch_wanted(&rt) => {
                rt.watch = Some(w);
                log::info!("搅拌机 开始监听粘贴键");
                None
            }
            // 装的这会儿结果没了：放锁后丢掉
            Some(w) => Some(w),
            // 没有辅助功能权限时不提示：贴出后不自动收起，用户自己关
            None => {
                if xpaste_platform::accessibility_trusted() {
                    log::warn!("搅拌机 粘贴键监听装不上");
                }
                None
            }
        }
    };
    drop(stale);
}

/// 粘贴键松开（`at_press` 是按下时的剪切板序号）：贴出的是结果时倒空杯子，抖一下、提示，再收起。
fn pasted(app: &AppHandle, at_press: u64) {
    let mut round = None;
    with(app, |_, rt, fx| {
        if !watch_wanted(rt) || at_press != rt.result_seq {
            return;
        }
        let items = rt.cup.clear();
        rt.again = Some((items, rt.recipe.clone(), Instant::now()));
        cancel(rt);
        reset_result(rt);
        rt.taken = None;
        rt.cleared = None;
        log::info!("搅拌机 贴出");
        if after_paste(rt.mini) == AfterPaste::Close {
            close(rt, fx, false);
            return;
        }
        round = Some(rt.round);
        fx.changed(Cause::Paste, None);
    });
    let Some(g) = round else {
        return;
    };
    std::thread::sleep(Duration::from_millis(PASTE_SHAKE_MS));
    with(app, |_, rt, fx| {
        if rt.round != g {
            return;
        }
        if let Some(a) = &mut rt.again {
            a.2 = Instant::now();
        }
        fx.toasts.push(toast("贴好了", &["again"]));
        fx.shake = true;
        fx.changed(Cause::Other, None);
    });
    std::thread::sleep(Duration::from_millis(PASTE_HIDE_MS));
    with(app, |_, rt, fx| {
        // 期间又放进了东西、点了再搅一次或关掉了：不收
        if rt.round != g || !rt.open || !rt.cup.items().is_empty() {
            return;
        }
        rt.open = false;
        fx.linger = true;
        fx.changed(Cause::Other, None);
        log::info!("搅拌机 贴出后收起");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_closes_when_mini() {
        assert_eq!(after_paste(true), AfterPaste::Close);
        assert_eq!(after_paste(false), AfterPaste::ShakeThenClose);
    }

    #[test]
    fn late_result_is_dropped() {
        assert_eq!(result_action(5, 4, true, true), ResultAction::Drop);
        assert_eq!(result_action(5, 5, true, true), ResultAction::Ready);
        assert_eq!(result_action(5, 5, false, true), ResultAction::Partial);
        assert_eq!(result_action(5, 5, false, false), ResultAction::Fail);
    }

    #[test]
    fn reason_texts() {
        use crate::translate::Fail;
        assert_eq!(reason(&Fail::Http(401), false), "API Key 无效或没有权限");
        assert_eq!(reason(&Fail::Http(500), false), "AI 接口出错（HTTP 500）");
        assert_eq!(reason(&Fail::Timeout, false), "AI 接口超时");
        assert_eq!(
            reason(&Fail::Http(400), true),
            "这个模型看不了图片，换一个能看图的模型，或拿掉图片再搅"
        );
        assert_eq!(reason(&Fail::Http(400), false), "AI 接口出错（HTTP 400）");
    }

    #[test]
    fn encode_shrinks_huge_image() {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::new(8000, 1000));
        let (mime, bytes) = encode_image(img).unwrap();
        assert_eq!(mime, "image/png");
        let back = image::load_from_memory(&bytes).unwrap();
        assert_eq!((back.width(), back.height()), (2048, 256));
    }

    #[test]
    fn encode_falls_back_to_jpeg() {
        // xorshift 噪点：PNG 压不下去
        let mut x: u32 = 0x9E37_79B9;
        let img = image::RgbaImage::from_fn(1600, 1600, |_, _| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            image::Rgba(x.to_le_bytes())
        });
        let (mime, bytes) = encode_image(image::DynamicImage::ImageRgba8(img)).unwrap();
        assert_eq!(mime, "image/jpeg");
        assert!(bytes.len() <= IMAGE_MAX_BYTES, "{} 字节", bytes.len());
    }

    #[test]
    fn own_result_not_taken() {
        assert!(!takes_copy(9, 9));
    }

    #[test]
    fn plain_own_write_taken() {
        // 截图“复制”等普通写入的序号和结果的序号不同
        assert!(takes_copy(10, 9));
    }

    #[test]
    fn expired_accepts_nothing_new() {
        assert!(!accepts_new(&Pro::Expired { reason: None }));
        assert!(!accepts_new(&Pro::Unavailable));
        assert!(accepts_new(&Pro::Trial { days_left: 3 }));
    }

    #[test]
    fn refuse_copy() {
        assert_eq!(
            Refuse::TooLong(Some(30_000)).text(),
            "文件太长（30000 字），最多 20000 字，可以复制其中一段再放"
        );
        assert_eq!(
            Refuse::TooLong(None).text(),
            "文件太长，最多 20000 字，可以复制其中一段再放"
        );
        assert_eq!(Refuse::Full.text(), "杯子满了（8 样），先搅一搅");
    }

    fn text_of(r: Piece) -> (String, String) {
        match r {
            Ok((IngredientKind::File, title, Body::Text(t))) => (title, t),
            Ok(_) => panic!("不是文本文件原料"),
            Err(e) => panic!("被拒：{e:?}"),
        }
    }

    #[test]
    fn file_ingredients() {
        let dir = std::env::temp_dir().join(format!("xpaste-mixer-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = |n: &str| dir.join(n);
        // GBK 的“中文”
        std::fs::write(p("a.log"), [0xD6, 0xD0, 0xCE, 0xC4]).unwrap();
        std::fs::write(p("b.tsx"), "const a = 1;\n").unwrap();
        std::fs::write(p("c.zip"), b"PK\x03\x04\x00\x00\x00\x00").unwrap();
        std::fs::write(p("d.txt"), "字".repeat(30_000)).unwrap();
        image::RgbaImage::new(1, 1).save(p("e.png")).unwrap();
        std::fs::write(p("f.txt"), vec![b'a'; 1_048_577]).unwrap();
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("log 中文".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        std::fs::write(p("g.txt"), utf16).unwrap();
        // 大于 1 MB 的二进制：按“这种文件”拒，不按“太长”
        let mut big = b"PK\x03\x04\x00\x00".to_vec();
        big.resize(1_048_577, 0x41);
        std::fs::write(p("h.docx"), big).unwrap();

        assert_eq!(
            text_of(read_file(1, &p("a.log"))),
            ("a.log".into(), "中文".into())
        );
        assert_eq!(
            text_of(read_file(1, &p("b.tsx"))),
            ("b.tsx".into(), "const a = 1;\n".into())
        );
        assert_eq!(read_file(1, &p("c.zip")).err(), Some(Refuse::Unsupported));
        assert_eq!(
            read_file(1, &p("d.txt")).err(),
            Some(Refuse::TooLong(Some(30_000)))
        );
        assert_eq!(read_file(1, &p("f.txt")).err(), Some(Refuse::TooLong(None)));
        assert_eq!(
            text_of(read_file(1, &p("g.txt"))),
            ("g.txt".into(), "log 中文".into())
        );
        assert_eq!(read_file(1, &p("h.docx")).err(), Some(Refuse::Unsupported));
        assert_eq!(read_file(1, &dir).err(), Some(Refuse::Unsupported));
        assert_eq!(read_file(1, &p("gone.txt")).err(), Some(Refuse::Missing));
        match read_file(7, &p("e.png")) {
            Ok((IngredientKind::Image, title, Body::Image { record_id, path })) => {
                assert_eq!(title, "e.png");
                assert_eq!(record_id, 7);
                assert_eq!(path.as_deref(), Some(p("e.png").as_path()));
            }
            _ => panic!("图片文件应当是图片原料"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
