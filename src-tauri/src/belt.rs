//! 传送带运行时：先攒一批，再按顺序一件件贴出去。只在带 Pro 的构建里有东西可放，带子不落盘。
//!
//! 剪切板里始终放着“下一件”；监听到别的应用里松开粘贴键后，按剪切板序号判断这次贴的是不是它：
//! 是就换下一件，不是（中间被别的写入改掉了）就只把“下一件”写回去，带子不前进。
//!
//! 锁顺序：`RT` 在最外层，持有它时可以短暂去拿 `settings`、`store`、`line`，反过来不行；
//! 持有 `RT` 时不发事件、不上主线程、不注册快捷键、不装卸按键监听：这些都攒进 `Fx`，放锁后由 `apply` 做。
//! 写剪切板在 `RT` 里做，这样采集线程的 `on_change` 会等到 `belt_seq` 记好以后再比较，不会把自己的写入当成让出。

use std::sync::atomic::Ordering;
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
use xpaste_core::Kind;
use xpaste_pro::{BELT_MAX, Belt, BeltError, BeltItem, Cleared, Pro, Removed, split_offer};

use crate::commands::records::{precheck, write_marked, write_text_plain};
use crate::events::{BELT_CHANGED, BELT_PUT_SELECTED, BELT_TOAST, emit, emit_to};
use crate::panel::{Rect, on_main};
use crate::state::AppState;

pub const LABEL: &str = "belt";

/// 松开粘贴键后等这么久再看剪切板序号、换下一件：目标应用要先把剪切板读完
pub const SWAP_MS: u64 = 120; // ponytail: 估的值；真机上慢的网页表单还会贴错的话，加长
/// 自动模式：这么长时间里复制满 `PEEK_MIN` 次就探出
const PEEK_WINDOW_MS: i64 = 30_000;
const PEEK_MIN: usize = 3;
/// 探出后点“开始”最多排上最近这么多次复制
const PEEK_CAP: usize = 20;
/// 探出后这么久没理会就缩回、计数清零
const PEEK_HOLD_MS: u64 = 8000;
/// 贴完以后还能退回的时长
const GRACE_MS: u64 = 4000;
/// 贴完以后过这么久收起（页面播完收起、提示消失后才藏窗口）
const FINISH_HIDE_MS: u64 = 700;

#[derive(Debug, PartialEq)]
pub enum PasteAction {
    /// 贴出去的是“下一件”：换下一件
    Advance,
    /// 剪切板被别的写入改过，这次贴的是那段内容：带子不前进，只把“下一件”写回去
    Yielded,
}

/// 按下粘贴键那一刻的剪切板序号还是写“下一件”时的那个，就算贴出了“下一件”。
pub fn paste_action(current_seq: u64, belt_seq: u64) -> PasteAction {
    if current_seq == belt_seq {
        PasteAction::Advance
    } else {
        PasteAction::Yielded
    }
}

/// 这次粘贴之后剪切板怎么处理
#[derive(Debug, PartialEq)]
pub enum Rewrite {
    /// 剪切板里已经是“下一件”
    Skip,
    /// 马上写“下一件”
    Now,
    /// 剪切板里是采集线程还没读的新内容：等它读完（`captured`）再写，不然这次复制进不了历史
    AfterCapture,
}

/// 贴出（或让出）之后要不要写“下一件”。`now`：此刻的剪切板序号；`seen`：采集线程读完的最后一个序号。
/// 剪切板还是自己写的、或者采集线程已经读过它，才能马上覆盖。
pub fn rewrite(action: &PasteAction, now: u64, belt_seq: u64, seen: u64) -> Rewrite {
    if now == belt_seq {
        // 让出时剪切板还是写“下一件”时的那次：不用重写（比如连按两次，第一次已经换好了）
        match action {
            PasteAction::Advance => Rewrite::Now,
            PasteAction::Yielded => Rewrite::Skip,
        }
    } else if now == seen {
        Rewrite::Now
    } else {
        Rewrite::AfterCapture
    }
}

/// 时间窗内的复制次数，满 `PEEK_MIN` 次时返回它。
pub fn peek_count(copies_ms: &[i64], now_ms: i64) -> Option<usize> {
    let n = copies_ms
        .iter()
        .filter(|&&t| now_ms - t <= PEEK_WINDOW_MS)
        .count();
    (n >= PEEK_MIN).then_some(n)
}

/// 这次入库的复制要不要放上带子：xPaste 自己按普通写入写的（截图复制、识别文字、译文、录屏复制）不放。
pub fn take_on_belt(seq: u64, own_write_seq: u64) -> bool {
    seq != own_write_seq
}

/// 试用中、已授权时能打开；试用结束后只有带子上还有东西时才行（把剩下的贴完）。
pub fn can_open(pro: &Pro, has_items: bool) -> bool {
    match pro {
        Pro::Trial { .. } | Pro::Licensed { .. } => true,
        Pro::Expired { .. } => has_items,
        Pro::Unavailable => false,
    }
}

/// 能不能往带子上放新东西：试用结束后不行。
fn accepts_new(pro: &Pro) -> bool {
    matches!(pro, Pro::Trial { .. } | Pro::Licensed { .. })
}

/// 传送带是否可用：与晾衣绳同一个构建条件。
pub fn enabled() -> bool {
    crate::line::enabled()
}

struct Runtime {
    /// 设置里的总开关与开启方式（自动时会探出）
    on: bool,
    auto: bool,
    belt: Belt,
    open: bool,
    /// 自动模式下探出了一截，还没正式开始
    peek: bool,
    peek_count: usize,
    /// 剪切板被别的写入改掉了，下一次粘贴先让给它
    yielding: bool,
    /// 写“下一件”之后的剪切板序号；写失败时为 `u64::MAX`，下一次粘贴按让出处理、重写
    belt_seq: u64,
    /// 采集线程读完的最后一个序号（含跳过的）
    seen: u64,
    /// 粘贴后该写“下一件”，但剪切板里的新内容采集线程还没读：读完后（`captured`）再写
    reclaim: bool,
    /// 最近一次写进剪切板的件；“下一件”换了才重写
    written: Option<u64>,
    /// 自动模式统计用：（复制时刻毫秒，记录 id），最多 `PEEK_CAP` 条
    copies: Vec<(i64, i64)>,
    /// 贴完以后可以退回的截止时刻
    grace_until: Option<Instant>,
    /// 贴完了、还没收起（`FINISH_HIDE_MS` 之内）
    finishing: bool,
    /// 收起计时、探出计时的代数：期间又有变化时旧的计时作废
    round: u64,
    peek_gen: u64,
    /// 带子打开且非空时持有
    watch: Option<xpaste_platform::PasteKeyWatch>,
    /// 这次打开以来贴出的件数
    pasted: u32,
    removed: Option<Removed>,
    cleared: Option<Cleared>,
    warned_expired: bool,
    warned_grant: bool,
}

static RT: LazyLock<Mutex<Runtime>> = LazyLock::new(|| {
    Mutex::new(Runtime {
        on: false,
        auto: false,
        belt: Belt::new(),
        open: false,
        peek: false,
        peek_count: 0,
        yielding: false,
        belt_seq: u64::MAX,
        seen: u64::MAX,
        reclaim: false,
        written: None,
        copies: Vec::new(),
        grace_until: None,
        finishing: false,
        round: 0,
        peek_gen: 0,
        watch: None,
        pasted: 0,
        removed: None,
        cleared: None,
        warned_expired: false,
        warned_grant: false,
    })
});

fn rt() -> MutexGuard<'static, Runtime> {
    RT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 已注册的退回快捷键。只在读写这个值时短暂持锁，注册、注销时不持锁（插件要等主线程，快捷键回调也在主线程读它）
static BACK: Mutex<Option<String>> = Mutex::new(None);

fn back_key() -> MutexGuard<'static, Option<String>> {
    BACK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 页面报来的可点区域（窗口内逻辑像素），点击穿透用
pub(crate) static HITS: Mutex<Vec<Rect>> = Mutex::new(Vec::new());

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Cause {
    Put,
    Paste,
    Remove,
    Split,
    Back,
    Pick,
    Reorder,
    Clear,
    Other,
}

/// `belt-changed` 的载荷，也是 `belt_state` 的返回值
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BeltState {
    open: bool,
    peek: bool,
    peek_count: usize,
    yielding: bool,
    items: Vec<BeltItem>,
    next: Option<u64>,
    can_back: bool,
    pro: Pro,
    cause: Cause,
    cause_id: Option<u64>,
    finished: Option<u32>,
}

/// `belt-toast` 的载荷：`actions` 是提示条上的按钮，`id` 是相关的那件
#[derive(Clone, Serialize, PartialEq, Debug)]
pub struct Toast {
    text: String,
    actions: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
}

fn toast(text: impl Into<String>, actions: &[&'static str], id: Option<u64>) -> Toast {
    Toast {
        text: text.into(),
        actions: actions.to_vec(),
        id,
    }
}

/// 问要不要拆开的提示
fn split_toast(n: usize, cells: bool, id: u64) -> Toast {
    let text = if cells {
        format!("这一行有 {n} 格，拆成 {n} 件？")
    } else {
        format!("这一段有 {n} 行，拆成 {n} 件？")
    };
    toast(text, &["split", "keepWhole"], Some(id))
}

enum Timer {
    /// 贴完：过一会儿收起，宽限期过了忘掉退回栈
    Finish(u64),
    /// 探出：没理会就缩回
    Peek(u64),
}

/// 持有 `RT` 期间攒下的、要放锁后再做的事
#[derive(Default)]
struct Fx {
    changed: Option<(Cause, Option<u64>)>,
    finished: Option<u32>,
    toasts: Vec<Toast>,
    show: bool,
    /// 收起：页面播完收起、等提示条消失后放手（`release`）才藏窗口，超时自己藏
    linger: bool,
    /// 马上藏窗口
    hide: bool,
    timer: Option<Timer>,
}

impl Fx {
    fn changed(&mut self, cause: Cause, id: Option<u64>) {
        self.changed = Some((cause, id));
    }
}

fn now_s() -> i64 {
    crate::clock::now_ms() / 1000
}

fn pro(app: &AppHandle) -> Pro {
    app.state::<AppState>().line().status(now_s())
}

fn snapshot(
    app: &AppHandle,
    cause: Cause,
    cause_id: Option<u64>,
    finished: Option<u32>,
) -> BeltState {
    let pro = pro(app);
    let rt = rt();
    BeltState {
        open: rt.open,
        peek: rt.peek,
        peek_count: rt.peek_count,
        yielding: rt.yielding,
        items: rt.belt.items().to_vec(),
        next: rt.belt.next().map(|i| i.id),
        can_back: rt.belt.can_back(),
        pro,
        cause,
        cause_id,
        finished,
    }
}

/// 放锁后做 `fx` 里攒下的事，再让按键监听和退回快捷键跟上带子此刻的状态。不要在持有 `RT` 时调用。
fn apply(app: &AppHandle, mut fx: Fx) {
    // 带子收着时的提示条（试用结束、已清空）：先把窗口显示出来，提示消失后页面放手
    if !fx.toasts.is_empty() && !fx.hide && closed() {
        fx.show = true;
        fx.linger = true;
    }
    for t in fx.toasts {
        emit(app, BELT_TOAST, t);
    }
    if let Some((cause, id)) = fx.changed {
        emit(app, BELT_CHANGED, snapshot(app, cause, id, fx.finished));
    }
    if fx.show {
        show(app);
    }
    if fx.hide {
        hide(app);
    } else if fx.linger {
        crate::belt_window::linger(app);
    }
    match fx.timer {
        Some(Timer::Finish(g)) => spawn(app, "belt-finish", move |app| finish_timer(app, g)),
        Some(Timer::Peek(g)) => spawn(app, "belt-peek", move |app| peek_timer(app, g)),
        None => {}
    }
    sync_watch(app);
    sync_back_key(app);
}

fn spawn(app: &AppHandle, name: &str, f: impl FnOnce(&AppHandle) + Send + 'static) {
    let app = app.clone();
    if let Err(e) = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || f(&app))
    {
        log::warn!("传送带线程 {name} 启动失败：{e}");
    }
}

/// 在 `RT` 里改一下带子，放锁后做攒下的事。
fn with(app: &AppHandle, f: impl FnOnce(&AppState, &Pro, &mut Runtime, &mut Fx)) {
    let state = app.state::<AppState>();
    let pro = pro(app);
    let mut fx = Fx::default();
    f(&state, &pro, &mut rt(), &mut fx);
    apply(app, fx);
}

/// 显示带子窗口（不拿键盘），收起；窗口的事见 `belt_window`。
pub fn show(app: &AppHandle) {
    crate::belt_window::show(app);
}

pub fn hide(app: &AppHandle) {
    crate::belt_window::hide(app);
}

/// 带子收着、也没探出：窗口该藏起来了
pub fn closed() -> bool {
    let rt = rt();
    !rt.open && !rt.peek
}

/// 页面放手时藏不藏窗口：带子开着或探出着（收起途中又打开了）就不藏
fn release_hides(open: bool, peek: bool) -> bool {
    !open && !peek
}

/// 页面播完收起、提示条也消失了：带子仍收着就藏窗口。只管窗口，不改带子的状态。
pub fn release(app: &AppHandle) {
    with(app, |_, _, rt, fx| {
        fx.hide = release_hides(rt.open, rt.peek);
    });
}

/// 贴完 0.7 s 后的收起：不再收东西、不再监听粘贴键；剪切板留着最后贴出的那件，宽限期里还能退回（会重新打开）。
/// 窗口等页面播完、提示消失后放手再藏。
fn collapse(rt: &mut Runtime, fx: &mut Fx) {
    rt.finishing = false;
    rt.open = false;
    fx.linger = true;
    fx.changed(Cause::Other, None);
}

/// 启动时调用（只在带 Pro 的构建里）：预建隐藏的窗口。
pub fn start(app: &AppHandle) {
    if enabled() {
        crate::belt_window::prewarm(app);
    }
    log::info!("传送带就绪");
}

/// 启动和保存设置时调用：记下总开关和开启方式。关掉时带子直接清空、收起，不弹撤销；
/// 退回快捷键按新设置重新注册（`hotkey::register` 注销全部时会先调 `forget_back`）。
pub fn configure(app: &AppHandle, on: bool, auto: bool) {
    with(app, |_, _, rt, fx| {
        rt.on = on && enabled();
        rt.auto = auto;
        if !rt.auto || !rt.on {
            rt.copies.clear();
        }
        if !rt.on {
            if rt.open || rt.peek {
                close(rt, fx, false);
            }
            rt.grace_until = None;
        }
    });
}

/// `hotkey::register` 注销全部快捷键以后调用：退回快捷键已经不在了。
pub fn forget_back() {
    *back_key() = None;
}

/// 传送带快捷键按下（主线程）：面板在屏上且拿着键盘时让面板把选中的放上来；否则开关带子（探出时正式开始）。
pub fn hotkey_pressed(app: &AppHandle) {
    if crate::panel::visible(app) && crate::panel::holds_key(app) {
        emit_to(app, "panel", BELT_PUT_SELECTED, ());
        return;
    }
    // 开始时要读库、写剪切板，不占着主线程
    spawn(app, "belt-hotkey", toggle);
}

fn toggle(app: &AppHandle) {
    with(app, |state, pro, rt, fx| {
        if !rt.on {
            return;
        }
        if rt.peek {
            begin(state, pro, rt, fx);
        } else if rt.open {
            close(rt, fx, true);
        } else if can_open(pro, false) {
            open(rt, fx);
            fx.changed(Cause::Other, None);
        } else {
            fx.toasts.push(expired_toast());
        }
    });
}

fn expired_toast() -> Toast {
    toast("试用已结束，传送带要 xPaste Pro", &["license", "buy"], None)
}

/// 退回快捷键按下（主线程）：是它就在别的线程退回一件，返回 true，不再按别的快捷键分派。
pub fn back_pressed(app: &AppHandle, pressed: &Shortcut) -> bool {
    let is = back_key()
        .as_deref()
        .is_some_and(|k| k.parse::<Shortcut>().is_ok_and(|s| s == *pressed));
    if is {
        spawn(app, "belt-back", back);
    }
    is
}

fn open(rt: &mut Runtime, fx: &mut Fx) {
    rt.open = true;
    rt.peek = false;
    rt.peek_count = 0;
    rt.copies.clear();
    rt.pasted = 0;
    rt.belt.forget_done();
    rt.grace_until = None;
    rt.finishing = false;
    rt.yielding = false;
    rt.reclaim = false;
    rt.written = None;
    rt.removed = None;
    rt.cleared = None;
    rt.warned_expired = false;
    rt.warned_grant = false;
    rt.round += 1;
    fx.show = true;
    log::info!("传送带 打开");
}

/// 收起；带子上还有东西时清空，`undo` 时提示可以撤销。
fn close(rt: &mut Runtime, fx: &mut Fx, undo: bool) {
    let n = rt.belt.items().len();
    if n > 0 {
        let c = rt.belt.clear();
        if undo {
            rt.cleared = Some(c);
            fx.toasts
                .push(toast(format!("已清空 {n} 件"), &["undoClear"], None));
        }
    }
    rt.open = false;
    rt.peek = false;
    rt.peek_count = 0;
    rt.copies.clear();
    rt.finishing = false;
    rt.yielding = false;
    rt.reclaim = false;
    rt.written = None;
    rt.removed = None;
    rt.round += 1;
    fx.linger = true;
    fx.changed(Cause::Clear, None);
    log::info!("传送带 收起 清空 {n} 件");
}

/// 从探出状态正式开始：时间窗内的复制按先后排上去。
fn begin(state: &AppState, pro: &Pro, rt: &mut Runtime, fx: &mut Fx) {
    if !can_open(pro, false) {
        fx.toasts.push(expired_toast());
        return;
    }
    let now = crate::clock::now_ms();
    let ids: Vec<i64> = rt
        .copies
        .iter()
        .filter(|c| now - c.0 <= PEEK_WINDOW_MS)
        .map(|c| c.1)
        .collect();
    open(rt, fx);
    let n = ids
        .iter()
        .filter(|&&id| put_record(state, rt, fx, id, false).is_some())
        .count();
    log::info!("传送带 开始 放上 {n} 件");
    write_next(state, rt, fx, true);
    fx.changed(Cause::Put, None);
}

/// 把记录 `id` 放到带子末尾：文字类存全文，图片、文件、视频只记 id。记录已删时不放。
/// `offer` 时多行文字顺带问要不要拆开。
fn put_record(
    state: &AppState,
    rt: &mut Runtime,
    fx: &mut Fx,
    id: i64,
    offer: bool,
) -> Option<u64> {
    let read = || -> Result<Option<(Kind, String, Option<String>)>, xpaste_core::CoreError> {
        let store = state.store();
        let Some(s) = store.live_summary(id)? else {
            return Ok(None);
        };
        let text = match s.kind {
            Kind::Text | Kind::Code | Kind::Link => store.text(id)?,
            _ => None,
        };
        Ok(Some((s.kind, s.title, text)))
    };
    let (kind, title, text) = match read() {
        Ok(Some(r)) => r,
        Ok(None) => {
            log::info!("传送带 记录已删除，不放上 id={id}");
            return None;
        }
        Err(e) => {
            log::warn!("传送带 读取记录 {id} 失败：{e}");
            return None;
        }
    };
    let whole = text.clone();
    match rt.belt.push(kind, title, Some(id), text) {
        Ok(item) => {
            rt.finishing = false;
            rt.grace_until = None;
            rt.cleared = None;
            if offer
                && let Some(t) = whole
                && let Some(s) = split_offer(&t, BELT_MAX - rt.belt.items().len() + 1)
            {
                fx.toasts.push(split_toast(s.parts.len(), s.cells, item));
            }
            Some(item)
        }
        Err(BeltError::Full) => {
            fx.toasts.push(toast(
                format!("传送带满了（{BELT_MAX} 件），先贴掉一些"),
                &[],
                None,
            ));
            None
        }
        Err(BeltError::NotFound) => None,
    }
}

/// 把“下一件”写进剪切板（`force` 时即使已经写过也重写，比如刚被别的复制改掉）。
/// 原记录没了（删掉了、文件找不到）的件拿掉、提示一次，接着试下一件。写入不算一次使用。
fn write_next(state: &AppState, rt: &mut Runtime, fx: &mut Fx, force: bool) {
    let mut skipped = false;
    while let Some(item) = rt.belt.next() {
        let (id, text, record) = (item.id, item.text.clone(), item.record_id);
        if !force && rt.written == Some(id) {
            break;
        }
        // 文字类写内存里的全文；图片、文件、视频按记录写回原格式
        let flavors = match (text.is_some(), record) {
            (true, _) => Some(Vec::new()),
            (false, Some(rid)) => live_flavors(state, rid),
            (false, None) => None,
        };
        let Some(flavors) = flavors else {
            if rt.belt.remove(id).is_err() {
                break;
            }
            log::info!("传送带 原记录没了，跳过 id={id}");
            skipped = true;
            continue;
        };
        let written = match &text {
            Some(t) => write_text_plain(state, t),
            None => write_marked(state, || xpaste_platform::write(&flavors)),
        };
        match written {
            Ok(()) => {
                rt.belt_seq = state.last_write_seq.load(Ordering::SeqCst);
                rt.written = Some(id);
                rt.yielding = false;
                rt.reclaim = false;
                log::info!("传送带 换下一件 id={id}");
            }
            Err(e) => {
                // 剪切板里不是“下一件”：下一次粘贴按让出处理，再重写
                rt.belt_seq = u64::MAX;
                rt.written = None;
                log::warn!("传送带 写下一件失败 id={id}：{e}");
            }
        }
        break;
    }
    if skipped {
        fx.toasts.push(toast("原记录已删除，跳过了一件", &[], None));
        // 贴出途中跳过的保留“贴出”这个原因，页面照常播贴出
        if fx.changed.is_none() {
            fx.changed(Cause::Remove, None);
        }
    }
}

/// 记录还在（没被删、没被清理）、文件也都在时，取写回剪切板用的格式。
fn live_flavors(state: &AppState, rid: i64) -> Option<Vec<xpaste_core::Flavor>> {
    let alive = matches!(state.store().live_summary(rid), Ok(Some(_)));
    alive.then(|| precheck(state, rid).ok()).flatten()
}

/// 采集回调的第一件事（每次剪切板变化，含 xPaste 自己的写入）：带子开着时，不是写“下一件”造成的变化就让出一次。
pub fn on_change(app: &AppHandle, seq: u64) {
    {
        let mut rt = rt();
        if !rt.open || rt.yielding || seq == rt.belt_seq || rt.belt.items().is_empty() {
            return;
        }
        rt.yielding = true;
    }
    emit(app, BELT_CHANGED, snapshot(app, Cause::Other, None, None));
}

/// 采集线程读完一次变化（记进历史或跳过）之后：粘贴时留着没写的“下一件”这时写回去。
/// 每次变化都调，没有要写的时候拿一下锁就返回。
pub fn captured(app: &AppHandle, seq: u64) {
    {
        let mut rt = rt();
        rt.seen = seq;
        if !rt.reclaim {
            return;
        }
    }
    with(app, |state, _, rt, fx| {
        // 又有更新的变化没读：留给它读完以后
        if !rt.reclaim || xpaste_platform::change_count() != seq {
            return;
        }
        rt.reclaim = false;
        if rt.open && !rt.belt.items().is_empty() {
            write_next(state, rt, fx, true);
            fx.changed(Cause::Other, None);
        }
    });
}

/// 一次复制记进了历史（新增或合并）之后：带子开着就放上去，没开着、自动模式下数一数要不要探出。
pub fn on_recorded(app: &AppHandle, id: i64, kind: Kind, seq: u64) {
    let state = app.state::<AppState>();
    if !take_on_belt(seq, state.own_write_seq.load(Ordering::SeqCst)) {
        return;
    }
    with(app, |state, pro, rt, fx| {
        if !rt.on {
            return;
        }
        if rt.open {
            if !accepts_new(pro) {
                if !std::mem::replace(&mut rt.warned_expired, true) {
                    fx.toasts
                        .push(toast("试用已结束，带子上只能贴完剩下的", &[], None));
                }
                return;
            }
            // 没放上（满了）时剪切板留着刚复制的，让出一次
            if let Some(item) = put_record(state, rt, fx, id, true) {
                log::info!("传送带 放上 1 件 id={id} kind={kind:?}");
                write_next(state, rt, fx, true);
                fx.changed(Cause::Put, Some(item));
            }
        } else if rt.auto && can_open(pro, false) {
            let now = crate::clock::now_ms();
            rt.copies.retain(|c| now - c.0 <= PEEK_WINDOW_MS);
            if rt.copies.len() >= PEEK_CAP {
                rt.copies.remove(0);
            }
            rt.copies.push((now, id));
            let times: Vec<i64> = rt.copies.iter().map(|c| c.0).collect();
            if let Some(n) = peek_count(&times, now) {
                if !rt.peek {
                    log::info!("传送带 探出");
                    fx.show = true;
                }
                rt.peek = true;
                rt.peek_count = n;
                rt.peek_gen += 1;
                fx.timer = Some(Timer::Peek(rt.peek_gen));
                fx.changed(Cause::Other, None);
            }
        }
    });
}

fn peek_timer(app: &AppHandle, g: u64) {
    std::thread::sleep(Duration::from_millis(PEEK_HOLD_MS));
    with(app, |_, _, rt, fx| {
        if rt.peek && rt.peek_gen == g {
            rt.peek = false;
            rt.peek_count = 0;
            rt.copies.clear();
            fx.linger = true;
            fx.changed(Cause::Other, None);
            log::info!("传送带 探出没理会，缩回");
        }
    });
}

/// 按键监听的回调（macOS 在主线程，Windows 在钩子线程），松开 V 时来：`at_press` 是按下 V 那一刻的
/// 剪切板序号（目标应用按下时就读剪切板，贴出去的是它）。只起线程，等目标应用读完剪切板再换下一件。
fn on_paste(app: &AppHandle, at_press: u64) {
    spawn(app, "belt-paste", move |app| {
        std::thread::sleep(Duration::from_millis(SWAP_MS));
        pasted(app, at_press);
    });
}

fn pasted(app: &AppHandle, at_press: u64) {
    with(app, |state, _, rt, fx| {
        if !rt.open || rt.belt.items().is_empty() {
            return;
        }
        // 在 RT 里读：读到的变化要么已经 `captured` 过（seen 跟上了），要么之后才 `captured`、会看到 reclaim
        let now = xpaste_platform::change_count();
        let action = paste_action(at_press, rt.belt_seq);
        let rw = rewrite(&action, now, rt.belt_seq, rt.seen);
        if action == PasteAction::Advance {
            let Some(item) = rt.belt.take_next() else {
                return;
            };
            rt.pasted += 1;
            log::info!("传送带 贴出 1 件 id={}", item.id);
            fx.changed(Cause::Paste, Some(item.id));
            if rt.belt.items().is_empty() {
                return finish(rt, fx);
            }
        } else {
            log::info!("传送带 让出一次");
            fx.changed(Cause::Other, None);
        }
        match rw {
            Rewrite::Skip => rt.yielding = false,
            Rewrite::Now => write_next(state, rt, fx, true),
            Rewrite::AfterCapture => rt.reclaim = true,
        }
    });
}

/// 贴完最后一件：剪切板留着它，提示可以退回，0.7 s 后收起；宽限期里退回快捷键还在。
fn finish(rt: &mut Runtime, fx: &mut Fx) {
    rt.finishing = true;
    rt.round += 1;
    rt.grace_until = Some(Instant::now() + Duration::from_millis(GRACE_MS));
    fx.finished = Some(rt.pasted);
    fx.toasts
        .push(toast(format!("贴了 {} 件", rt.pasted), &["back"], None));
    fx.timer = Some(Timer::Finish(rt.round));
    log::info!("传送带 贴完 {} 件", rt.pasted);
}

fn finish_timer(app: &AppHandle, g: u64) {
    std::thread::sleep(Duration::from_millis(FINISH_HIDE_MS));
    with(app, |_, _, rt, fx| {
        if finish_due(rt.finishing, rt.round, g) {
            collapse(rt, fx);
        }
    });
    std::thread::sleep(Duration::from_millis(GRACE_MS - FINISH_HIDE_MS));
    with(app, |_, _, rt, fx| {
        if !rt.open && rt.grace_until.is_some_and(|t| Instant::now() >= t) {
            rt.grace_until = None;
            rt.belt.forget_done();
            fx.changed(Cause::Other, None);
        }
    });
}

/// 贴完的收起计时到了时还该不该收起：期间退回了、又放上了（`finishing` 清掉、`round` 变了）就不收
fn finish_due(finishing: bool, round: u64, g: u64) -> bool {
    finishing && round == g
}

/// 退回刚贴出的一件，它重新当“下一件”；贴完收起后的宽限期里会重新打开带子。
pub fn back(app: &AppHandle) {
    with(app, |state, _, rt, fx| {
        if !rt.open && rt.grace_until.is_none() {
            return;
        }
        let Some(id) = rt.belt.back() else {
            return;
        };
        if !rt.open || rt.finishing {
            rt.open = true;
            rt.finishing = false;
            rt.round += 1;
            fx.show = true;
        }
        rt.grace_until = None;
        rt.pasted = rt.pasted.saturating_sub(1);
        log::info!("传送带 退回 id={id}");
        write_next(state, rt, fx, false);
        fx.changed(Cause::Back, Some(id));
    });
}

/// 按键监听跟上带子：打开且非空时装上，其余时候卸掉。装不上（macOS 没有辅助功能权限）时每次打开提示一次；
/// 下次放上、打开时再试（权限可能已经给了）。
fn sync_watch(app: &AppHandle) {
    let wanted = |rt: &Runtime| rt.open && !rt.belt.items().is_empty();
    let (want, has) = {
        let rt = rt();
        (wanted(&rt), rt.watch.is_some())
    };
    if want == has {
        return;
    }
    if !want {
        // 放锁后再丢：Windows 上要等钩子线程退出
        let old = rt().watch.take();
        drop(old);
        log::info!("传送带 停止监听粘贴键");
        return;
    }
    // macOS 必须在主线程装
    on_main(app, move |app| {
        {
            let rt = rt();
            if !wanted(&rt) || rt.watch.is_some() {
                return;
            }
        }
        let a = app.clone();
        let watch = xpaste_platform::watch_paste_key(move |seq| on_paste(&a, seq));
        let grant = {
            let mut rt = rt();
            match watch {
                Some(w) if wanted(&rt) => {
                    rt.watch = Some(w);
                    log::info!("传送带 开始监听粘贴键");
                    false
                }
                // 装的这会儿带子收了：放锁后随 `watch` 丢掉
                Some(_) => false,
                None => {
                    let need = !xpaste_platform::accessibility_trusted();
                    if !need {
                        log::warn!("传送带 粘贴键监听装不上");
                    }
                    need && !std::mem::replace(&mut rt.warned_grant, true)
                }
            }
        };
        if grant {
            emit(
                app,
                BELT_TOAST,
                toast("按顺序贴需要辅助功能权限", &["grant"], None),
            );
        }
    });
}

/// 退回快捷键跟上带子：打开时、贴完的宽限期里注册，其余时候注销，让给别的应用。
fn sync_back_key(app: &AppHandle) {
    let active = {
        let rt = rt();
        rt.on && (rt.open || rt.grace_until.is_some())
    };
    let want = active
        .then(|| app.state::<AppState>().settings().belt_back_hotkey.clone())
        .filter(|k| !k.is_empty());
    let old = {
        let mut b = back_key();
        if *b == want {
            return;
        }
        std::mem::replace(&mut *b, want.clone())
    };
    let gs = app.global_shortcut();
    if let Some(k) = old
        && let Err(e) = gs.unregister(k.as_str())
    {
        log::debug!("注销退回快捷键失败：{e}");
    }
    if let Some(k) = want
        && let Err(e) = gs.register(k.as_str())
    {
        log::warn!("退回快捷键注册失败");
        log::debug!("全局快捷键插件错误：{e}");
        let mut b = back_key();
        if b.as_deref() == Some(k.as_str()) {
            *b = None;
        }
    }
}

// ---- 页面与面板调用的操作（`commands::belt`） ----

pub fn state_now(app: &AppHandle) -> BeltState {
    snapshot(app, Cause::Other, None, None)
}

pub fn ready(app: &AppHandle) {
    emit(app, BELT_CHANGED, state_now(app));
}

/// 窗口藏着时不收：藏起以后才到的旧报告不能把点击区域放回去（显示后页面会重报）
pub fn set_hits(rects: Vec<Rect>) {
    if !crate::belt_window::shown() {
        return;
    }
    *HITS.lock().unwrap_or_else(PoisonError::into_inner) = rects;
}

/// 面板里放上几条记录（按给的顺序排在末尾），带子没开就顺便打开。
pub fn put(app: &AppHandle, ids: &[i64]) {
    with(app, |state, pro, rt, fx| {
        if !rt.on {
            return;
        }
        if !accepts_new(pro) {
            fx.toasts.push(if rt.open {
                toast("试用已结束，带子上只能贴完剩下的", &[], None)
            } else {
                expired_toast()
            });
            return;
        }
        if !rt.open {
            open(rt, fx);
        }
        let mut last = None;
        let mut n = 0;
        for &id in ids {
            if let Some(item) = put_record(state, rt, fx, id, ids.len() == 1) {
                last = Some(item);
                n += 1;
            }
        }
        log::info!("传送带 放上 {n} 件");
        write_next(state, rt, fx, false);
        fx.changed(Cause::Put, last);
    });
}

/// 点一件：它成为“下一件”，剪切板马上换成它，不再让出。
pub fn pick(app: &AppHandle, id: u64) {
    with(app, |state, _, rt, fx| {
        if rt.belt.pick(id).is_ok() {
            write_next(state, rt, fx, true);
        }
        fx.changed(Cause::Pick, Some(id));
    });
}

pub fn reorder(app: &AppHandle, ids: &[u64]) {
    with(app, |_, _, rt, fx| {
        if rt.belt.reorder(ids).is_err() {
            log::info!("传送带 换顺序的 id 对不上，按现状重发");
        }
        fx.changed(Cause::Reorder, None);
    });
}

pub fn remove(app: &AppHandle, id: u64) {
    with(app, |state, _, rt, fx| {
        if let Ok(r) = rt.belt.remove(id) {
            rt.removed = Some(r);
            fx.toasts.push(toast("已拿掉", &["undoRemove"], Some(id)));
            log::info!("传送带 拿掉 id={id}");
            write_next(state, rt, fx, false);
        }
        fx.changed(Cause::Remove, Some(id));
    });
}

pub fn undo_remove(app: &AppHandle) {
    with(app, |state, _, rt, fx| {
        if !rt.open {
            return;
        }
        if let Some(r) = rt.removed.take() {
            rt.belt.restore(r);
            log::info!("传送带 撤销拿掉");
            write_next(state, rt, fx, false);
        }
        fx.changed(Cause::Put, None);
    });
}

/// 拆开提示的回答：`accept` 时按行（或按格）拆成几件，否则整段放着不动。
pub fn split(app: &AppHandle, id: u64, accept: bool) {
    with(app, |state, _, rt, fx| {
        if !accept {
            return;
        }
        let room = BELT_MAX - rt.belt.items().len() + 1;
        let parts = rt
            .belt
            .items()
            .iter()
            .find(|i| i.id == id)
            .and_then(|i| i.text.as_deref())
            .and_then(|t| split_offer(t, room));
        if let Some(s) = parts {
            let n = s.parts.len();
            if rt.belt.split(id, s.parts).is_ok() {
                log::info!("传送带 拆开 id={id} 成 {n} 件");
                write_next(state, rt, fx, false);
            }
        }
        fx.changed(Cause::Split, Some(id));
    });
}

pub fn close_now(app: &AppHandle) {
    with(app, |_, _, rt, fx| {
        if rt.open || rt.peek {
            close(rt, fx, true);
        }
    });
}

/// 撤销清空：按原来的顺序放回，“下一件”不变；带子已收起时重新打开。
pub fn undo_clear(app: &AppHandle) {
    with(app, |state, _, rt, fx| {
        if !rt.belt.items().is_empty() {
            return;
        }
        let Some(c) = rt.cleared.take() else {
            return;
        };
        if !rt.open {
            open(rt, fx);
        }
        rt.belt.unclear(c);
        log::info!("传送带 撤销清空 {} 件", rt.belt.items().len());
        write_next(state, rt, fx, false);
        fx.changed(Cause::Put, None);
    });
}

/// 探出时点了“开始”
pub fn begin_now(app: &AppHandle) {
    with(app, |state, pro, rt, fx| {
        if rt.on && rt.peek {
            begin(state, pro, rt, fx);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use xpaste_pro::Pro;

    #[test]
    fn advance_when_clipboard_is_ours() {
        assert_eq!(paste_action(7, 7), PasteAction::Advance);
    }

    #[test]
    fn yields_after_foreign_write() {
        // 面板 Enter、截图复制、复制密码以后
        assert_eq!(paste_action(8, 7), PasteAction::Yielded);
    }

    #[test]
    fn release_ignored_while_open_or_peeking() {
        assert!(!release_hides(true, false));
        assert!(!release_hides(false, true));
        assert!(release_hides(false, false));
    }

    #[test]
    fn finish_collapses_unless_backed_or_refilled() {
        assert!(finish_due(true, 5, 5));
        // 退回、又放上东西：finishing 清掉；退回重开还会让 round 变
        assert!(!finish_due(false, 5, 5));
        assert!(!finish_due(true, 6, 5));
    }

    #[test]
    fn peek_needs_three_in_window() {
        assert_eq!(peek_count(&[71_000, 80_000, 99_000], 100_000), Some(3));
        assert_eq!(peek_count(&[69_000, 80_000, 99_000], 100_000), None);
    }

    #[test]
    fn own_plain_write_not_taken() {
        assert!(!take_on_belt(9, 9));
        assert!(take_on_belt(9, 8));
    }

    #[test]
    fn expired_opens_only_with_leftovers() {
        let expired = Pro::Expired { reason: None };
        assert!(!can_open(&expired, false));
        assert!(can_open(&expired, true));
        assert!(can_open(&Pro::Trial { days_left: 3 }, false));
    }

    #[test]
    fn two_presses_within_swap_second_yields() {
        // 两次都在换件之前松开，看到的都是 1；第一次换好后 belt_seq 成了 2
        assert_eq!(paste_action(1, 1), PasteAction::Advance);
        assert_eq!(rewrite(&PasteAction::Advance, 1, 1, 1), Rewrite::Now);
        assert_eq!(paste_action(1, 2), PasteAction::Yielded);
        // 剪切板里已经是新的“下一件”，不用重写
        assert_eq!(rewrite(&PasteAction::Yielded, 2, 2, 1), Rewrite::Skip);
    }

    #[test]
    fn press_before_swap_released_after_yields() {
        // 第二次在换件前按下（序号 1）、换件后（belt_seq 成了 2）才松开：目标贴的是第一件，不消耗第二件
        assert_eq!(paste_action(1, 2), PasteAction::Yielded);
        assert_eq!(rewrite(&PasteAction::Yielded, 2, 2, 2), Rewrite::Skip);
    }

    #[test]
    fn fresh_copy_not_overwritten_until_captured() {
        // 复制了 6 马上贴：采集线程还停在 5
        assert_eq!(paste_action(6, 5), PasteAction::Yielded);
        assert_eq!(
            rewrite(&PasteAction::Yielded, 6, 5, 5),
            Rewrite::AfterCapture
        );
        // 读完 6 以后再贴可以马上写
        assert_eq!(rewrite(&PasteAction::Yielded, 6, 5, 6), Rewrite::Now);
        // 贴出“下一件”后、换件前又复制了一次：也等采集读完
        assert_eq!(
            rewrite(&PasteAction::Advance, 6, 5, 5),
            Rewrite::AfterCapture
        );
    }

    #[test]
    fn failed_write_makes_next_paste_yield() {
        // 写失败后 belt_seq 为 u64::MAX
        assert_eq!(paste_action(7, u64::MAX), PasteAction::Yielded);
        assert_eq!(rewrite(&PasteAction::Yielded, 7, u64::MAX, 7), Rewrite::Now);
    }

    #[test]
    fn split_toast_copy() {
        let t = split_toast(5, false, 1);
        assert_eq!(t.text, "这一段有 5 行，拆成 5 件？");
        assert_eq!(t.actions, ["split", "keepWhole"]);
        assert_eq!(split_toast(4, true, 1).text, "这一行有 4 格，拆成 4 件？");
    }
}
