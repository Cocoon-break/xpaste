//! 划词翻译卡片（M26 规则 18、19）：模拟复制取词、定位、显示卡片窗口 `tr-card`、翻译。
//! 卡片窗口启动时预建、隐藏；macOS 是不激活应用的 NSPanel（同截图遮罩，M23 规则 1），拿键盘但前台应用不变。
//! 日志不写原文、译文，只记 copied、字数、耗时与错误类别（M26 规则 14）。

use std::sync::atomic::Ordering;
use std::sync::{MutexGuard, PoisonError};
use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use xpaste_core::capture;
use xpaste_platform::PlatformError;

use crate::error::AppError;
use crate::events::{self, TRANSLATE_DELTA, TRCARD_DONE, TRCARD_SHOW};
use crate::panel::{Rect, on_main};
use crate::state::AppState;
use crate::translate::{self, Coalescer, Delta, Engine, Lang};

pub const LABEL: &str = "tr-card";
/// 离指针的距离
const GAP: f64 = 12.0;
/// 卡片初始尺寸（逻辑像素）；前端渲染后经 `trcard_resize` 改高度，展开对照时改宽度
const CARD: (f64, f64) = (360.0, 160.0);
/// 最高为工作区高度的 60%
const MAX_H: f64 = 0.6;
/// 展开对照（原文、译文左右两栏）：宽 760（不超过工作区），最高为工作区高度的 80%
const WIDE: (f64, f64) = (760.0, 0.8);

/// 卡片的位置与所在屏（逻辑像素）。`up`：放在指针上方（向上翻转），改高度时底边不动。
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub up: bool,
    pub work: Rect,
    /// 所在屏的缩放（Windows 按它换成物理像素）
    pub sf: f64,
}

/// 划词卡片会话（`AppState::trcard`）。每次按快捷键、关闭、重翻都换 `seq`，旧请求据此中止且不再发事件。
#[derive(Debug, Default)]
pub struct Session {
    seq: u64,
    /// 本次要翻译的原文；空表示没有（排除应用、无文字、已关闭）
    text: String,
    place: Option<Placement>,
    /// Windows：按快捷键时的前台窗口，关闭时把键盘还给它（macOS 恒为 None，orderOut 时系统自己交还）
    previous: Option<isize>,
    /// 按快捷键时面板有焦点：关闭时把键盘还给面板
    panel_focused: bool,
    /// 卡片拿了键盘（`text` / `input` 上屏后）；提示不拿，撤下时不交还
    keyboard: bool,
    /// 输入卡片（M26 规则 20）：点别处不关闭，免得写了一半的字丢掉
    input: bool,
    /// 这张卡片上选的目标语言；None 为“自动”（按设置的目标语言，规则 9）
    lang: Option<Lang>,
}

/// 撤下卡片时键盘交还到哪（M26 规则 18、19）。
#[derive(Debug, PartialEq)]
enum Back {
    /// 不动：卡片没拿键盘，或 macOS 上由 orderOut 交还
    Stay,
    Panel,
    /// Windows：按快捷键时的前台窗口
    Window(isize),
}

impl Session {
    /// 卡片没拿过键盘（提示）时哪也不还：键盘一直在用户手里，交还会把前台抢回他可能已离开的应用（设计反目标“不抢前台”）。
    /// 按快捷键时面板有焦点、面板仍开着时还给面板，否则还给原来的窗口。
    fn back(&self, panel_shown: bool) -> Back {
        match self.previous {
            _ if !self.keyboard => Back::Stay,
            _ if self.panel_focused && panel_shown => Back::Panel,
            Some(h) => Back::Window(h),
            None => Back::Stay,
        }
    }

    pub fn bump(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    pub fn is_current(&self, seq: u64) -> bool {
        self.seq == seq
    }
}

/// 当前序号（后端自己关闭卡片时用）。
pub fn current(app: &AppHandle) -> u64 {
    session(app).seq
}

fn session(app: &AppHandle) -> MutexGuard<'_, Session> {
    app.state::<AppState>()
        .inner()
        .trcard
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// 卡片左上角：放在指针右下方 `GAP`；右边放不下翻到指针左侧，下边放不下翻到指针上方；最后夹在工作区内
/// （两侧都放不下、比工作区还大时贴工作区左上）。
pub fn place_card(cursor: (f64, f64), card: (f64, f64), work: Rect) -> (f64, f64) {
    let axis = |c: f64, len: f64, lo: f64, span: f64| {
        let after = c + GAP;
        let v = if after + len > lo + span {
            c - GAP - len
        } else {
            after
        };
        // 先 min 后 max（不用 clamp）：卡片比工作区大时贴左上而不是 panic
        v.min(lo + span - len).max(lo)
    };
    (
        axis(cursor.0, card.0, work.x, work.w),
        axis(cursor.1, card.1, work.y, work.h),
    )
}

/// 改成高度 `h`（不超过工作区的 60%，展开对照时 80%）、按 `wide` 换宽度：向上翻转的卡片底边不动，否则顶边不动；
/// 左边不动，右边越过工作区时整体往左推；再夹回工作区。
fn resized(p: Placement, h: f64, wide: bool) -> Placement {
    let (w, max_h) = if wide {
        (WIDE.0.min(p.work.w), WIDE.1)
    } else {
        (CARD.0, MAX_H)
    };
    let h = h.min(p.work.h * max_h).max(1.0);
    let y = if p.up { p.y + p.h - h } else { p.y };
    let y = y.min(p.work.y + p.work.h - h).max(p.work.y);
    let x = p.x.min(p.work.x + p.work.w - w).max(p.work.x);
    Placement { x, y, w, h, ..p }
}

/// `trcard-show` 的载荷。`kind`：`text`（有文字，开始翻译）/ `input`（空的输入卡片，M26 规则 20）/
/// `blocked`（排除应用或敏感内容）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Show {
    kind: &'static str,
    seq: u64,
    /// `text`：完整原文（卡片上只显示前 40 个字，展开对照时整段显示）
    source: Option<String>,
    /// `text`：翻译方式标签（`system` / `openai:{模型}`）
    engine: Option<String>,
    /// `text`：没有辅助功能权限，翻译的是剪切板里的文字
    no_access: bool,
}

impl Show {
    /// 翻译卡片与输入卡片拿键盘；提示 2 秒后自己消失，不打断打字。
    fn takes_keyboard(&self) -> bool {
        self.kind != "blocked"
    }

    /// 没有原文的卡片：`blocked` 提示或空的输入卡片。
    fn bare(kind: &'static str, seq: u64) -> Self {
        Self {
            kind,
            seq,
            source: None,
            engine: None,
            no_access: false,
        }
    }
}

/// `trcard-done` 的载荷：这次翻译结束。`lang`：成功时实际翻成的语言代码（规则 9）。
/// `error` 同命令报错的 `{ kind, message, source?, target? }`。
#[derive(Debug, Clone, Serialize)]
struct Done {
    seq: u64,
    ok: bool,
    text: Option<String>,
    lang: Option<String>,
    engine: String,
    error: Option<serde_json::Value>,
}

/// `trcard_translate` 的结果：新的序号（恒为传入的 + 1）与这次用的翻译方式标签。
#[derive(Debug, Serialize)]
pub struct Again {
    pub seq: u64,
    pub engine: String,
}

/// 启动时建好隐藏的卡片窗口，免得按快捷键时现建 webview。
pub fn prewarm(app: &AppHandle) {
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("card.html".into()))
        .title("xpaste-card")
        .theme(crate::panel::current_theme(app))
        .visible(false)
        .focused(false)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .resizable(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .accept_first_mouse(true)
        .inner_size(CARD.0, CARD.1)
        .build();
    match built {
        #[cfg(target_os = "macos")]
        Ok(w) => on_main(app, move |_| {
            // 层级同贴图（NSFloatingWindowLevel）
            if let Err(e) = crate::shot::to_overlay_panel(&w, 3) {
                log::warn!("划词卡片转换 NSPanel 失败：{e}");
            }
        }),
        #[cfg(not(target_os = "macos"))]
        Ok(_) => {}
        Err(e) => log::warn!("创建划词卡片失败：{e}"),
    }
}

/// 卡片窗口可见。
pub fn shown(app: &AppHandle) -> bool {
    app.get_webview_window(LABEL)
        .is_some_and(|w| w.is_visible().unwrap_or(false))
}

/// 卡片可见且拿着键盘：面板此时失焦不隐藏（M26 规则 18：面板开着时按快捷键，面板不隐藏）。
/// 不拿键盘的提示不算，面板失焦给别的应用时照常隐藏。
pub fn has_keyboard(app: &AppHandle) -> bool {
    app.get_webview_window(LABEL)
        .is_some_and(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false))
}

/// 翻译快捷键按下、不交给面板录制框时调用。主线程调用：只换序号、撤下开着的卡片，模拟复制（最多约 900 ms）
/// 与翻译都在后台做。
pub fn pressed(app: &AppHandle) {
    let seq = reset(app);
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || grab(&app, seq));
}

/// 开一张新卡片前：换序号、记下键盘要还去的地方，开着的卡片先撤下。返回新序号。主线程调用。
fn reset(app: &AppHandle) -> u64 {
    let was_shown = shown(app);
    let panel_shown = panel_shown(app);
    let panel_focused = panel_shown
        && app
            .get_webview_window("panel")
            .is_some_and(|w| w.is_focused().unwrap_or(false));
    let previous = xpaste_platform::foreground_window();
    let (seq, back) = {
        let mut s = session(app);
        s.text.clear();
        let card_has_keyboard = was_shown && s.keyboard;
        let back = if card_has_keyboard {
            s.back(panel_shown)
        } else {
            Back::Stay
        };
        // 拿着键盘的卡片开着时再按：前台是卡片自己，沿用上一次记下的去处；提示开着时前台仍是用户手里的应用
        if !card_has_keyboard {
            s.previous = previous;
            s.panel_focused = panel_focused;
        }
        s.keyboard = false;
        s.lang = None;
        (s.bump(), back)
    };
    // 卡片开着时先撤下：拿着键盘的话先交还（同关闭），模拟复制才发得到原来的应用。前端随后的 blur 带着旧序号，被忽略
    if was_shown {
        hide_to(app, back);
    }
    seq
}

/// 后台线程：排除应用 → 模拟复制 → 读剪切板文字 → 显示卡片并开始翻译。
fn grab(app: &AppHandle, seq: u64) {
    let state = app.state::<AppState>();
    let (ignored, skip_sensitive) = {
        let s = state.settings();
        (s.ignored_apps.clone(), s.skip_sensitive)
    };
    // 排除应用（含密码管理器，M15）：不复制、不翻译
    if xpaste_platform::frontmost_app().is_some_and(|a| capture::is_ignored_app(&a.name, &ignored))
    {
        log::info!("trcard 排除应用");
        return show(app, Show::bare("blocked", seq));
    }
    // 等修饰键松开期间又按了一次：只让新的那次发模拟复制
    if !session(app).is_current(seq) {
        return;
    }
    let t0 = Instant::now();
    // 失败一律改为翻译剪切板；只有没权限时卡片带提示（M26 规则 18）
    let (copied, no_access) = match xpaste_platform::copy_selection() {
        Ok(c) => (c, false),
        Err(PlatformError::NoAccessibility) => (false, true),
        Err(e) => {
            log::warn!("trcard 模拟复制失败：{e}");
            (false, false)
        }
    };
    let snap = xpaste_platform::read_snapshot(skip_sensitive).unwrap_or_else(|e| {
        log::warn!("trcard 读取剪切板失败：{e}");
        None
    });
    // 敏感内容（M01）：不翻译，免得把密码发给翻译接口
    if snap.as_ref().is_some_and(|s| s.sensitive) {
        log::info!("trcard 排除应用 sensitive");
        return show(app, Show::bare("blocked", seq));
    }
    let text = snap.and_then(|s| s.text).filter(|t| !t.trim().is_empty());
    let chars = text.as_deref().map_or(0, |t| t.chars().count());
    log::info!(
        "trcard copied={copied} chars={chars} ms={}",
        t0.elapsed().as_millis()
    );
    // 选区和剪切板都没有文字：打开空的输入卡片（M26 规则 20）
    let Some(text) = text else {
        log::info!("trcard 无文字，打开输入卡片");
        return show(app, Show::bare("input", seq));
    };
    let engine = translate::cfg(&state.settings()).tag();
    {
        let mut s = session(app);
        if !s.is_current(seq) {
            return;
        }
        s.text.clone_from(&text);
    }
    show(
        app,
        Show {
            kind: "text",
            seq,
            source: Some(text.clone()),
            engine: Some(engine),
            no_access,
        },
    );
}

/// 托盘菜单“翻译…”：在鼠标旁打开空的输入卡片（M26 规则 20）。开着的卡片先撤下（同再按翻译快捷键）。主线程调用。
pub fn open_input(app: &AppHandle) {
    let seq = reset(app);
    show(app, Show::bare("input", seq));
}

/// 定位到指针旁、发 `trcard-show`、上屏。已被新的一次取代时什么都不做。
/// `text` 拿键盘并开始翻译：翻译排在 `trcard-show` 之后开始，增量与结果不会先于它到达、被它清掉；
/// `input` 拿键盘、等用户输入；`blocked` 提示不拿键盘，用户接着打的字照常进原来的应用。
fn show(app: &AppHandle, payload: Show) {
    on_main(app, move |app| {
        let Some(w) = app.get_webview_window(LABEL) else {
            return log::warn!("找不到划词卡片窗口");
        };
        {
            let mut s = session(app);
            if !s.is_current(payload.seq) {
                return;
            }
            s.keyboard = payload.takes_keyboard();
            s.input = payload.kind == "input";
        }
        match locate(app) {
            Ok(Some(p)) => {
                session(app).place = Some(p);
                if let Err(e) = apply(&w, &p) {
                    log::warn!("划词卡片定位失败：{e}");
                }
            }
            Ok(None) => log::warn!("划词卡片找不到显示器"),
            Err(e) => log::warn!("划词卡片取指针位置失败：{e}"),
        }
        let (seq, start, focus) = (
            payload.seq,
            payload.kind == "text",
            payload.takes_keyboard(),
        );
        events::emit_to(app, LABEL, TRCARD_SHOW, payload);
        if start {
            tauri::async_runtime::spawn(run(app.clone(), seq, None));
        }
        if let Err(e) = front(app, &w, focus) {
            log::warn!("显示划词卡片失败：{e}");
        }
    });
}

/// 指针位置与所在屏的工作区（避开菜单栏、程序坞、任务栏），按初始尺寸算出位置。主线程调用。
fn locate(app: &AppHandle) -> tauri::Result<Option<Placement>> {
    let c = app.cursor_position()?;
    let Some(m) = crate::panel::cursor_monitor(app)? else {
        return Ok(None);
    };
    let sf = m.scale_factor();
    // 换算同 panel::place 与 panel::monitor_contains：macOS 的光标按主屏缩放、显示器按各自缩放换回点；
    // Windows 都是物理像素，按所在屏缩放换成逻辑像素
    let cs = if cfg!(target_os = "macos") {
        app.primary_monitor()?.map_or(1.0, |p| p.scale_factor())
    } else {
        sf
    };
    let wa = m.work_area();
    let work = Rect {
        x: wa.position.x as f64 / sf,
        y: wa.position.y as f64 / sf,
        w: wa.size.width as f64 / sf,
        h: wa.size.height as f64 / sf,
    };
    let cursor = (c.x / cs, c.y / cs);
    let (x, y) = place_card(cursor, CARD, work);
    Ok(Some(Placement {
        x,
        y,
        w: CARD.0,
        h: CARD.1,
        up: y < cursor.1,
        work,
        sf,
    }))
}

/// 摆窗口：macOS 用逻辑坐标（点），Windows 按所在屏缩放换成物理像素（见 `shot::place`）。
fn apply(w: &WebviewWindow, p: &Placement) -> tauri::Result<()> {
    let k = if cfg!(target_os = "macos") { 1.0 } else { p.sf };
    crate::shot::place(w, (p.x * k, p.y * k), (p.w * k, p.h * k))
}

/// 上屏，`focus` 时拿键盘；macOS 上不激活应用，前台应用保持不变（同 `shot::show_overlay`）。主线程调用。
fn front(app: &AppHandle, w: &WebviewWindow, focus: bool) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use tauri_nspanel::ManagerExt;
        match app.get_webview_panel(LABEL) {
            // orderFrontRegardless + makeKeyWindow
            Ok(p) if focus => p.show(),
            Ok(p) => p.order_front_regardless(),
            Err(_) => w.show()?,
        }
        if focus {
            w.as_ref().set_focus()?;
        }
    }
    #[cfg(windows)]
    {
        let _ = app;
        if focus {
            w.show()?;
            w.set_focus()?;
        } else {
            // show() 是会激活窗口的 SW_SHOW，提示要不抢前台
            xpaste_platform::show_no_activate(w.hwnd()?.0 as isize);
        }
    }
    Ok(())
}

/// 撤下卡片（NSPanel 上 hide 即 orderOut，键盘交还仍处于激活状态的前台应用）。
/// 所有收起卡片的路径（再按快捷键、关闭、失焦、Alt+F4）都走这里。
fn hide_window(app: &AppHandle) {
    on_main(app, |app| {
        let Some(w) = app.get_webview_window(LABEL) else {
            return;
        };
        // 隐藏前取消挂在卡片里的语言包下载，否则那边一直等、下载标记卡住（M26 规则 17）
        #[cfg(target_os = "macos")]
        if let Ok(ns) = w.ns_window() {
            xpaste_platform::translate_prepare_cancel(ns as usize);
        }
        // 保留 hide()：`text` 卡片经 show() 上屏，tao 的可见标记要跟着复位
        if let Err(e) = w.hide() {
            log::warn!("隐藏划词卡片失败：{e}");
        }
        // Windows：提示经 show_no_activate 上屏，tao 的标记仍是隐藏，上面的 hide() 不会真的隐藏
        #[cfg(windows)]
        match w.hwnd() {
            Ok(h) => xpaste_platform::hide_window(h.0 as isize),
            Err(e) => log::warn!("划词卡片取不到窗口句柄：{e}"),
        }
    });
}

/// 撤下卡片，键盘交还到 `back`。Windows：卡片还在前台时先切回原来的窗口（本进程此刻是前台进程，系统允许），再撤下卡片。
fn hide_to(app: &AppHandle, back: Back) {
    if let Back::Window(h) = back {
        on_main(app, move |_| xpaste_platform::activate_window(h));
    }
    hide_window(app);
    if back == Back::Panel {
        crate::panel::refocus(app);
    }
}

fn panel_shown(app: &AppHandle) -> bool {
    app.get_webview_window("panel")
        .is_some_and(|w| w.is_visible().unwrap_or(false))
}

/// 翻译会话原文，增量只发给卡片；被新的一次取代时中止请求、不再发事件。
async fn run(app: AppHandle, seq: u64, engine: Option<Engine>) {
    let (text, lang) = {
        let s = session(&app);
        if !s.is_current(seq) {
            return;
        }
        (s.text.clone(), s.lang)
    };
    let mut cfg = translate::cfg_for(&app.state::<AppState>().settings(), engine);
    if let Some(l) = lang {
        cfg.lang = l;
    }
    let tag = cfg.tag();
    let current = || session(&app).is_current(seq);
    let send = |l: Lang, text: String| {
        let p = Delta {
            id: None,
            lang: l.code().into(),
            engine: tag.clone(),
            text,
        };
        events::emit_to(&app, LABEL, TRANSLATE_DELTA, p);
    };
    let t0 = Instant::now();
    let mut co = Coalescer::default();
    let mut lang = None;
    let r = translate::exec(&cfg, &text, None, |l, d| {
        if !current() {
            return false;
        }
        lang = Some(l);
        if let Some(s) = co.push(d, Instant::now()) {
            send(l, s);
        }
        true
    })
    .await;
    let ms = t0.elapsed().as_millis();
    let chars = text.chars().count();
    if !current() {
        log::info!("translate card 中止 chars={chars} ms={ms}");
        return;
    }
    // 剩下的发完（失败时也发：卡片保留已收到的部分）
    if let (Some(l), Some(s)) = (lang, co.flush()) {
        send(l, s);
    }
    let done = match r {
        Ok((l, out)) => {
            log::info!(
                "translate card engine={} chars={chars} ms={ms} ok",
                cfg.engine.name()
            );
            Done {
                seq,
                ok: true,
                text: Some(out),
                lang: Some(l.code().into()),
                engine: tag,
                error: None,
            }
        }
        Err(f) => {
            log::info!("translate card 失败 {} chars={chars} ms={ms}", f.tag());
            Done {
                seq,
                ok: false,
                text: None,
                lang: None,
                engine: tag,
                error: serde_json::to_value(AppError::from(f)).ok(),
            }
        }
    };
    events::emit_to(&app, LABEL, TRCARD_DONE, done);
}

/// 用卡片上的原文 `text` 与目标语言 `lang`（None 为自动）翻译：输入卡片按 ⌘↵、改了原文重翻、换用、
/// 下载语言包后重试（M26 规则 16、20）；`engine` 为 None 时按设置。翻译中调用时中止进行中的请求。
/// 不是当前这一次（卡片已关或已被取代）时报错。卡片重新拿键盘：下载语言包时系统确认框可能拿走了它。
pub fn translate(
    app: &AppHandle,
    seq: u64,
    text: String,
    lang: Option<Lang>,
    engine: Option<Engine>,
) -> Result<Again, AppError> {
    if text.trim().is_empty() {
        return Err(AppError::Invalid("没有可翻译的文字".into()));
    }
    let seq = {
        let mut s = session(app);
        if !s.is_current(seq) {
            return Err(AppError::Invalid("卡片已关闭".into()));
        }
        s.text = text;
        s.lang = lang;
        s.bump()
    };
    let engine_tag = translate::cfg_for(&app.state::<AppState>().settings(), engine).tag();
    take_keyboard(app);
    tauri::async_runtime::spawn(run(app.clone(), seq, engine));
    Ok(Again {
        seq,
        engine: engine_tag,
    })
}

/// 下载语言包的系统确认框取消或失败后，卡片重新拿键盘（期间失焦被忽略，不拿回来 Esc 和点别处都不再生效，M26 规则 17）。
/// 不是当前这一次时忽略。
pub fn focus(app: &AppHandle, seq: u64) {
    if session(app).is_current(seq) {
        take_keyboard(app);
    }
}

fn take_keyboard(app: &AppHandle) {
    on_main(app, |app| {
        if let Some(w) = app.get_webview_window(LABEL)
            && let Err(e) = front(app, &w, true)
        {
            log::warn!("划词卡片取得焦点失败：{e}");
        }
    });
}

/// 前端渲染后按内容改高度，`wide` 为展开对照（见 `resized`）。不是当前这一次时忽略。
/// 用户拖动过卡片（标题行是拖动区，规则 19）时以窗口现在的位置与所在屏为准，不弹回原处，之后顶边不动。
pub fn resize(app: &AppHandle, seq: u64, h: f64, wide: bool) {
    on_main(app, move |app| {
        let Some(w) = app.get_webview_window(LABEL) else {
            return;
        };
        let now = current_place(&w);
        let p = {
            let mut s = session(app);
            if !s.is_current(seq) {
                return;
            }
            let Some(p) = s.place.as_mut() else { return };
            if let Some((x, y, work, sf)) = now
                && ((x - p.x).abs() > 1.0 || (y - p.y).abs() > 1.0)
            {
                *p = Placement {
                    x,
                    y,
                    work,
                    sf,
                    up: false,
                    ..*p
                };
            }
            *p = resized(*p, h, wide);
            *p
        };
        if let Err(e) = apply(&w, &p) {
            log::warn!("划词卡片改大小失败：{e}");
        }
    });
}

/// 窗口现在的左上角（逻辑像素）与所在屏的工作区、缩放；换算同 `locate`。取不到时 None。
fn current_place(w: &WebviewWindow) -> Option<(f64, f64, Rect, f64)> {
    let m = w.current_monitor().ok()??;
    let sf = m.scale_factor();
    let pos = w.outer_position().ok()?;
    let wa = m.work_area();
    let work = Rect {
        x: wa.position.x as f64 / sf,
        y: wa.position.y as f64 / sf,
        w: wa.size.width as f64 / sf,
        h: wa.size.height as f64 / sf,
    };
    Some((pos.x as f64 / sf, pos.y as f64 / sf, work, sf))
}

/// 关闭（Esc、复制后、提示自动消失）：中止进行中的请求，撤下卡片；卡片拿过键盘时还给按快捷键前的地方
/// （面板或原来的应用；macOS 上原来的应用由 orderOut 交还），提示不动键盘与前台。不是当前这一次时忽略。
pub fn close(app: &AppHandle, seq: u64) {
    let panel_shown = panel_shown(app);
    let back = {
        let mut s = session(app);
        if !s.is_current(seq) {
            return;
        }
        s.bump();
        s.text.clear();
        let back = s.back(panel_shown);
        s.previous = None;
        s.keyboard = false;
        back
    };
    hide_to(app, back);
}

/// 卡片失去键盘（点了别处）：同关闭，但键盘已在用户点的地方，不再交还。面板开着且没开对话框时，
/// 像面板自己失焦一样隐藏。下载语言包期间忽略（系统确认框会拿走键盘）。不是当前这一次时忽略。
pub fn blur(app: &AppHandle, seq: u64) {
    let state = app.state::<AppState>();
    if state.tr_downloading.load(Ordering::SeqCst) {
        return;
    }
    {
        let mut s = session(app);
        // 输入卡片点别处不关（M26 规则 20）
        if !s.is_current(seq) || s.input {
            return;
        }
        s.bump();
        s.text.clear();
        s.previous = None;
        s.keyboard = false;
    }
    hide_window(app);
    if !state.dialog_open.load(Ordering::SeqCst) {
        crate::panel::hide_if_unfocused(app);
    }
}

/// 复制译文：按普通写入写纯文本，进历史（同截图卡片，M26 规则 19）。
pub fn copy(text: &str) -> Result<(), AppError> {
    let chars = text.chars().count();
    xpaste_platform::write(&[xpaste_core::text_flavor(text)]).map_err(|e| {
        log::warn!("trcard 译文写剪切板失败 {chars} 字：{e}");
        AppError::ClipboardWrite
    })?;
    log::info!("trcard 复制译文 {chars} 字");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect {
        x: 0.0,
        y: 25.0,
        w: 1440.0,
        h: 815.0,
    }; // 去掉菜单栏与程序坞

    #[test]
    fn place_card_flips_and_clamps() {
        let work = WORK;
        assert_eq!(
            place_card((100.0, 100.0), (360.0, 160.0), work),
            (112.0, 112.0)
        );
        assert_eq!(
            place_card((1400.0, 100.0), (360.0, 160.0), work),
            (1028.0, 112.0)
        ); // 右边放不下 → 指针左侧 12
        assert_eq!(
            place_card((100.0, 800.0), (360.0, 160.0), work),
            (112.0, 628.0)
        ); // 下边放不下 → 指针上方 12
        assert_eq!(
            place_card((1400.0, 800.0), (360.0, 160.0), work),
            (1028.0, 628.0)
        );
        // 两边都放不下（工作区窄于指针两侧）：夹在工作区内
        let narrow = Rect {
            x: 0.0,
            y: 0.0,
            w: 500.0,
            h: 300.0,
        };
        assert_eq!(
            place_card((250.0, 150.0), (360.0, 160.0), narrow),
            (0.0, 0.0)
        );
        // 比工作区还宽：贴左上，不 panic
        assert_eq!(place_card((10.0, 10.0), (600.0, 400.0), narrow), (0.0, 0.0));
    }

    /// seq 变了以后，旧请求的 on_delta 返回 false。
    #[test]
    fn stale_session_aborts() {
        let mut s = Session::default();
        let a = s.bump();
        assert!(s.is_current(a));
        let b = s.bump(); // 再按一次快捷键 / 关闭 / 重翻
        assert!(!s.is_current(a), "旧请求中止");
        assert!(s.is_current(b));
    }

    fn placed(y: f64, h: f64, up: bool) -> Placement {
        Placement {
            x: 112.0,
            y,
            w: 360.0,
            h,
            up,
            work: WORK,
            sf: 2.0,
        }
    }

    /// 改高度：向上翻转的卡片底边不动，否则顶边不动；最高为工作区的 60%，超出工作区时往回推。
    #[test]
    fn resize_keeps_flipped_edge() {
        let p = resized(placed(628.0, 160.0, true), 200.0, false);
        assert_eq!((p.y, p.h), (588.0, 200.0));
        let p = resized(placed(112.0, 160.0, false), 200.0, false);
        assert_eq!((p.y, p.h), (112.0, 200.0));
        let p = resized(placed(112.0, 160.0, false), 2000.0, false);
        assert_eq!(p.h, 815.0 * 0.6);
        let p = resized(placed(600.0, 160.0, false), 400.0, false);
        assert_eq!((p.y, p.h), (440.0, 400.0)); // 底边 840 不越过工作区
    }

    /// 展开对照：宽 760、最高 80%，右边越界时往左推；收起回到 360，左边不动。
    #[test]
    fn resize_wide_widens_and_clamps() {
        let p = resized(placed(112.0, 160.0, false), 2000.0, true);
        assert_eq!((p.x, p.w, p.h), (112.0, 760.0, 815.0 * 0.8));
        let p = resized(
            Placement {
                x: 1000.0,
                ..placed(112.0, 160.0, false)
            },
            300.0,
            true,
        );
        assert_eq!((p.x, p.w), (680.0, 760.0));
        let p = resized(p, 300.0, false);
        assert_eq!((p.x, p.w), (680.0, 360.0));
        let narrow = Placement {
            work: Rect { w: 600.0, ..WORK },
            ..placed(112.0, 160.0, false)
        };
        let p = resized(narrow, 300.0, true);
        assert_eq!((p.x, p.w), (0.0, 600.0));
    }

    /// 翻译卡片与输入卡片拿键盘；blocked 提示不打断打字。
    #[test]
    fn hints_do_not_take_keyboard() {
        assert!(!Show::bare("blocked", 1).takes_keyboard());
        assert!(Show::bare("input", 1).takes_keyboard());
        assert!(Show::bare("text", 1).takes_keyboard());
    }

    /// 撤下卡片时：提示（没拿键盘）哪也不还；拿着键盘时按快捷键前面板有焦点且面板仍开着 → 面板，否则 → 原来的窗口。
    #[test]
    fn back_only_when_card_had_keyboard() {
        let s = |keyboard, panel_focused, previous| Session {
            keyboard,
            panel_focused,
            previous,
            ..Session::default()
        };
        assert_eq!(
            s(false, false, Some(7)).back(true),
            Back::Stay,
            "提示不抢前台"
        );
        assert_eq!(
            s(false, true, Some(7)).back(true),
            Back::Stay,
            "提示不拉回面板"
        );
        assert_eq!(s(true, true, Some(7)).back(true), Back::Panel);
        assert_eq!(
            s(true, true, Some(7)).back(false),
            Back::Window(7),
            "面板已收起"
        );
        assert_eq!(s(true, false, Some(7)).back(true), Back::Window(7));
        assert_eq!(
            s(true, false, None).back(true),
            Back::Stay,
            "macOS 由 orderOut 交还"
        );
    }
}
