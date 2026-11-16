//! 搅拌机运行时：开着时每次记进历史的复制都掉进杯子，之后按配方让 AI 合成一条。只在带 Pro 的构建里有东西可放，杯子不落盘。
//!
//! 锁顺序同传送带：`RT` 在最外层，持有它时可以短暂去拿 `settings`、`line`，反过来不行；持有 `RT` 时不发事件、不读文件，
//! 要发的事件攒进 `Fx`，放锁后由 `apply` 发。传送带的 `RT` 和这里的 `RT` 从不同时持有。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use xpaste_core::Kind;
use xpaste_core::textfile::{TEXT_FILE_MAX_BYTES, decode_text, looks_binary};
use xpaste_pro::{Body, CUP_MAX, Cup, CupError, Ingredient, IngredientKind, MAX_CHARS, Pro, Taken};

use crate::collect::{self, Taker};
use crate::events::{MIXER_CHANGED, MIXER_PUT_SELECTED, MIXER_TOAST, emit_to};
use crate::panel::Rect;
use crate::state::AppState;

pub const LABEL: &str = "mixer";

/// 搅拌机是否编入此构建（与传送带同一个条件）。
pub fn enabled() -> bool {
    crate::line::enabled()
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
// 开搅以后的几个阶段由下一步的请求用到
#[allow(dead_code)]
pub enum Phase {
    Idle,
    Blending,
    Ready,
    Failed,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
// 开搅、停下、出结果、贴出由下一步的请求用到
#[allow(dead_code)]
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
    /// 打开时取的戳：和传送带同开时，新复制归后打开的那个
    opened: u64,
    cup: Cup,
    phase: Phase,
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
        opened: 0,
        cup: Cup::new(),
        phase: Phase::Idle,
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
    phase: Phase,
    items: Vec<Ingredient>,
    /// 打开的配方名，按顺序
    keys: Vec<String>,
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
        phase: rt.phase,
        items: rt.cup.items().to_vec(),
        keys,
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

/// 放锁后发 `fx` 里攒下的事件。不要在持有 `RT` 时调用。
fn apply(app: &AppHandle, fx: Fx) {
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

/// 启动时调用（调用方先看过 `enabled`）。窗口还没有，这里先什么都不做。
pub fn start(_app: &AppHandle) {}

/// 启动和保存设置时调用：记下总开关。关掉时直接倒掉、收起，不弹撤销。
pub fn configure(app: &AppHandle, on: bool) {
    with(app, |_, rt, fx| {
        rt.on = on && enabled();
        if !rt.on && rt.open {
            close(rt, fx, false);
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
        if rt.open {
            close(rt, fx, true);
        } else if accepts_new(pro) {
            open(rt);
            fx.changed(Cause::Other, None);
        } else {
            fx.toasts.push(expired_toast());
        }
    });
}

fn open(rt: &mut Runtime) {
    rt.open = true;
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
    rt.phase = Phase::Idle;
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
            open(rt);
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

pub fn set_hits(rects: Vec<Rect>) {
    *HITS.lock().unwrap_or_else(PoisonError::into_inner) = rects;
}

/// 页面播完收起、提示条也消失了。窗口还没有，这里先什么都不做。
pub fn release(_app: &AppHandle) {}

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
    with(app, |_, rt, fx| {
        if !rt.cup.items().is_empty() {
            return;
        }
        let Some(c) = rt.cleared.take() else {
            return;
        };
        if !rt.open {
            open(rt);
        }
        rt.cup.refill(c);
        log::info!("搅拌机 撤销倒掉 {} 样", rt.cup.items().len());
        fx.changed(Cause::Put, None);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
