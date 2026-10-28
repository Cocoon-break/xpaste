//! 截图会话与每屏遮罩窗口（设计 §4.1、§4.3）。
//!
//! 流程：快捷键 → `begin` 在后台线程冻结所有显示器 → 给每块屏的遮罩 `shot-<i>` 发 `shot-begin`
//! → 遮罩用 `shot_frame` 取冻结图、画好后 `shot_ready` 上屏 → 全部就绪后光标所在屏拿键盘焦点
//! → 遮罩合成 PNG 交给 `shot_finish` 做复制 / 保存 / 识别文字 / 贴图（§4.5）→ 提示条 → `shot_cancel`。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use xpaste_platform::{Frame, WinRect};

use crate::error::AppError;
use crate::events;
use crate::panel::on_main;
use crate::state::AppState;

const LABEL_PREFIX: &str = "shot-";
/// 按下快捷键后这么久还没全部上屏，视为卡住（某块屏的遮罩出错或没收到载荷），下次快捷键重新开始
const STALE: Duration = Duration::from_secs(3);

/// 显示器内的逻辑像素矩形（左上为原点）。
#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
pub struct LRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 窗口外框与该屏相交的部分，换成该屏的逻辑像素；不相交为 `None`。
/// `points`：原生单位已是点（macOS），否则是物理像素（Windows），要按该屏缩放换算。
pub fn to_local(r: &WinRect, f: &Frame, points: bool) -> Option<LRect> {
    let (x0, y0) = (r.x.max(f.origin.0), r.y.max(f.origin.1));
    let x1 = (r.x + r.w).min(f.origin.0 + f.size.0);
    let y1 = (r.y + r.h).min(f.origin.1 + f.size.1);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let k = if points { 1.0 } else { f.scale };
    Some(LRect {
        x: (x0 - f.origin.0) / k,
        y: (y0 - f.origin.1) / k,
        w: (x1 - x0) / k,
        h: (y1 - y0) / k,
    })
}

/// 光标（原生单位）落在哪块屏，以及在该屏内的逻辑像素坐标。
pub fn cursor_on(frames: &[Frame], c: (f64, f64), points: bool) -> Option<(usize, (f64, f64))> {
    frames.iter().enumerate().find_map(|(i, f)| {
        let (dx, dy) = (c.0 - f.origin.0, c.1 - f.origin.1);
        let k = if points { 1.0 } else { f.scale };
        (dx >= 0.0 && dy >= 0.0 && dx < f.size.0 && dy < f.size.1).then_some((i, (dx / k, dy / k)))
    })
}

pub enum BeginPlan {
    /// 没有屏幕录制权限：不显示遮罩，改由面板提示授权（设计 §4.8）
    NeedPermission,
    Freeze,
}

pub fn begin_plan(granted: bool) -> BeginPlan {
    if granted {
        BeginPlan::Freeze
    } else {
        BeginPlan::NeedPermission
    }
}

/// 一次截图的状态：从按下快捷键到遮罩全部隐藏。
#[derive(Default)]
pub struct Session {
    /// `Arc`：`shot_frame` 在锁外拷贝，不让几十 MB 的拷贝挡住其他屏和主线程
    frames: Vec<Arc<Frame>>,
    payloads: Vec<BeginPayload>,
    active: bool,
    /// 每次开始加一，后台冻结线程据此认出自己是否已过期
    generation: u64,
    started: Option<Instant>,
    /// 各屏遮罩是否已上屏
    ready: Vec<bool>,
    /// 光标所在屏，全部就绪后它拿键盘焦点
    pub cursor: Option<usize>,
    /// 截图前的前台窗口（Windows HWND），动作完成后把键盘还给它；macOS 恒为 `None`
    pub previous_window: Option<isize>,
    /// 某个动作正在执行（识别文字可能要几秒），同一次截图里的第二个动作被拒绝
    finishing: bool,
}

impl Session {
    /// 开始一次截图；已在进行中返回 false（连按快捷键只认第一次）。
    pub fn try_begin(&mut self) -> bool {
        self.try_begin_at(Instant::now())
    }

    /// 同 `try_begin`；进行中但超过 `STALE` 仍未全部上屏的会话视为已结束，重置后开始新的。
    pub fn try_begin_at(&mut self, now: Instant) -> bool {
        if self.active {
            let stale = !self.all_ready()
                && self
                    .started
                    .is_some_and(|t| now.saturating_duration_since(t) > STALE);
            if !stale {
                return false;
            }
            self.end();
        }
        self.active = true;
        self.generation += 1;
        self.started = Some(now);
        true
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 后台线程开始时记下的代数仍是当前这次截图。
    pub fn is_current(&self, generation: u64) -> bool {
        self.active && self.generation == generation
    }

    pub fn set_frames(&mut self, f: Vec<Frame>) {
        self.ready = vec![false; f.len()];
        self.frames = f.into_iter().map(Arc::new).collect();
    }

    pub fn frame(&self, i: usize) -> Option<Arc<Frame>> {
        self.frames.get(i).cloned()
    }

    pub fn set_payloads(&mut self, p: Vec<BeginPayload>) {
        self.payloads = p;
    }

    /// 第 `i` 块屏的 `shot-begin` 载荷；已上屏的屏返回 `None`，免得前端推拉两路各处理一次。
    pub fn payload(&self, i: usize) -> Option<BeginPayload> {
        if self.ready.get(i).copied().unwrap_or(true) {
            return None;
        }
        self.payloads.get(i).cloned()
    }

    /// 结束并释放冻结图（每屏几十 MB）；代数保留。返回还没交还键盘的截图前前台窗口。
    pub fn end(&mut self) -> Option<isize> {
        let previous = self.previous_window;
        *self = Self {
            generation: self.generation,
            ..Self::default()
        };
        previous
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    fn all_ready(&self) -> bool {
        !self.ready.is_empty() && self.ready.iter().all(|r| *r)
    }

    /// 记第 `i` 块屏已上屏；恰好让全部就绪的那一次返回 `Some(true)`，重复或越界的序号不算。
    /// `generation` 不是当前这次截图（上次的遮罩迟到的就绪）时返回 `None`，什么也不记。
    pub fn mark_ready(&mut self, generation: u64, i: usize) -> Option<bool> {
        if !self.is_current(generation) {
            return None;
        }
        Some(match self.ready.get_mut(i) {
            Some(r) if !*r => {
                *r = true;
                self.ready.iter().all(|r| *r)
            }
            _ => false,
        })
    }

    /// 认领本次截图的输出；已有动作在执行或已完成时返回 false。
    pub fn claim_finish(&mut self) -> bool {
        !std::mem::replace(&mut self.finishing, true)
    }

    /// 动作失败：放开认领，用户可以重试。
    pub fn release_finish(&mut self) {
        self.finishing = false;
    }

    /// 动作已完成，遮罩进入提示条模式：释放冻结图，会话保持进行中，提示条期间按快捷键不会把提示条截进去。
    /// 计时从现在重新算：前端没调 `shot_cancel` 时，`STALE` 之后的快捷键照常开始新的一次。
    /// 返回截图前的前台窗口并清掉它：键盘只还一次，之后 `shot_cancel` 不会把用户已切去的应用又切回来。
    pub fn finish(&mut self, now: Instant) -> Option<isize> {
        self.frames.clear();
        self.payloads.clear();
        self.ready.clear();
        self.started = Some(now);
        // 冻结图已释放，之后的 shot_finish 因取不到帧被拒绝
        self.finishing = false;
        self.previous_window.take()
    }
}

/// `shot-begin` 的载荷，前端 `src/api/types.ts` 与之对应。
#[derive(Serialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BeginPayload {
    index: usize,
    /// 本次截图的代数，前端调 `shot_ready` 时带回
    generation: u64,
    /// 与该屏相交的窗口，由前到后，供吸附
    rects: Vec<LRect>,
    /// 只有光标所在那块屏有值
    cursor: Option<(f64, f64)>,
    scale: f64,
    /// 设置“保存截图时同时复制”，工具栏提示用
    save_copy: bool,
}

pub fn overlay_index(label: &str) -> Option<usize> {
    label.strip_prefix(LABEL_PREFIX)?.parse().ok()
}

fn label(i: usize) -> String {
    format!("{LABEL_PREFIX}{i}")
}

pub fn session(app: &AppHandle) -> MutexGuard<'_, Session> {
    app.state::<AppState>()
        .inner()
        .shot
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// `shot_frame` 的响应体：小端 `u32` 宽、高，接着是 RGBA。
pub fn frame_bytes(f: &Frame) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8 + f.rgba.len());
    buf.extend_from_slice(&f.px.0.to_le_bytes());
    buf.extend_from_slice(&f.px.1.to_le_bytes());
    buf.extend_from_slice(&f.rgba);
    buf
}

fn timing() -> bool {
    crate::timing_enabled(std::env::var_os("XPASTE_TIMING"))
}

/// 系统设置“隐私与安全性”里屏幕录制那一页的名字：macOS 15 起改叫“录屏与系统录音”。
pub fn screen_capture_pane(macos_major: Option<u32>) -> &'static str {
    if macos_major.is_some_and(|v| v >= 15) {
        "录屏与系统录音"
    } else {
        "屏幕录制"
    }
}

/// 快捷键入口。主线程调用。
pub fn begin(app: &AppHandle) {
    if let BeginPlan::NeedPermission = begin_plan(xpaste_platform::screen_capture_granted()) {
        log::info!("截图缺少屏幕录制权限，提示授权");
        crate::panel::show(app, None, crate::panel::Trigger::Hotkey);
        events::emit_to(
            app,
            "panel",
            events::SHOT_NEED_PERMISSION,
            screen_capture_pane(xpaste_platform::macos_version()),
        );
        return;
    }
    let panel_shown = app
        .get_webview_window("panel")
        .is_some_and(|w| w.is_visible().unwrap_or(false));
    let (generation, stale) = {
        let mut s = session(app);
        let was_active = s.is_active();
        if !s.try_begin() {
            return;
        }
        // 面板开着时前台是面板自己（foreground_window 排除本进程），用面板唤起时记下的那个；
        // 面板没开时那个值可能早已过期，不用
        s.previous_window = xpaste_platform::foreground_window().or_else(|| {
            Some(
                app.state::<AppState>()
                    .previous_window
                    .load(Ordering::Relaxed),
            )
            .filter(|&h| panel_shown && h != 0)
        });
        (s.generation(), was_active)
    };
    // 上一次卡住的会话：已上屏的遮罩还盖在屏上，先撤下，不然会被截进去
    if stale {
        log::info!("shot 上次截图 {}s 内未全部上屏，重新开始", STALE.as_secs());
        hide_overlays(app);
    }
    // 面板不能被截进去
    if panel_shown {
        crate::panel::hide(app);
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || freeze(&app, generation, panel_shown || stale));
}

/// `settle`：刚隐藏过窗口（面板或上次的遮罩），冻结前先等它们撤下。
fn freeze(app: &AppHandle, generation: u64, settle: bool) {
    if settle {
        // ponytail: 固定等 50 ms 让窗口服务器撤下窗口；冻结图里仍出现面板时改为等隐藏生效的回调
        std::thread::sleep(Duration::from_millis(50));
    }
    let t = Instant::now();
    let frames = match xpaste_platform::freeze_displays() {
        Ok(f) if !f.is_empty() => f,
        Ok(_) => {
            log::warn!("截图时没有可冻结的显示器");
            return end_if_current(app, generation);
        }
        Err(e) => {
            log::warn!("冻结屏幕失败：{e}");
            return end_if_current(app, generation);
        }
    };
    let rects = xpaste_platform::window_rects();
    if timing() {
        log::info!(
            "shot 冻结 {} 屏 {}ms",
            frames.len(),
            t.elapsed().as_millis()
        );
    }
    let points = cfg!(target_os = "macos");
    // macOS 上 tao 的光标按主屏缩放成物理像素（见 panel::monitor_contains），除以主屏（frames[0]）缩放换回点
    let k = if points { frames[0].scale } else { 1.0 };
    let cursor = app
        .cursor_position()
        .ok()
        .and_then(|c| cursor_on(&frames, (c.x / k, c.y / k), points));
    let save_copy = app.state::<AppState>().settings().shot_save_copy;
    let payloads: Vec<BeginPayload> = frames
        .iter()
        .enumerate()
        .map(|(i, f)| BeginPayload {
            index: i,
            generation,
            rects: rects
                .iter()
                .filter_map(|r| to_local(r, f, points))
                .collect(),
            cursor: cursor.filter(|(j, _)| *j == i).map(|(_, p)| p),
            scale: f.scale,
            save_copy,
        })
        .collect();
    // 显示器比遮罩多（启动后接了新屏）：补建，等页面加载完。新遮罩仍可能赶不上下面推送的事件
    // （前端注册监听要走一次 IPC），所以载荷也存进会话，由前端注册好监听后用 `shot_begin_payload` 拉取。
    // 显示器变少时多出来的遮罩留着不用：tauri-nspanel 按标签持有 NSPanel，销毁后同名重建会重复释放
    for rx in ensure_overlays(app, frames.len()) {
        let _ = rx.recv_timeout(Duration::from_secs(5));
    }
    // 先摆好位置再放出载荷：前端拿到载荷就可能上屏
    for (i, f) in frames.iter().enumerate() {
        if let Some(w) = app.get_webview_window(&label(i))
            && let Err(e) = place(&w, f.origin, f.size)
        {
            log::warn!("摆放遮罩 {i} 失败：{e}");
        }
    }
    {
        let mut s = session(app);
        // 冻结期间被取消，或已被新的一次截图取代
        if !s.is_current(generation) {
            return;
        }
        s.cursor = cursor.map(|(i, _)| i);
        s.set_frames(frames);
        s.set_payloads(payloads.clone());
    }
    for p in payloads {
        events::emit_to(app, &label(p.index), events::SHOT_BEGIN, p);
    }
}

fn end_if_current(app: &AppHandle, generation: u64) {
    let mut s = session(app);
    if s.is_current(generation) {
        s.end();
    }
}

/// 遮罩铺满整块屏（含菜单栏）。
fn place(w: &WebviewWindow, origin: (f64, f64), size: (f64, f64)) -> tauri::Result<()> {
    // macOS 改内容尺寸时左下角不动，先改尺寸再定左上角
    #[cfg(target_os = "macos")]
    {
        w.set_size(tauri::LogicalSize::new(size.0, size.1))?;
        w.set_position(tauri::LogicalPosition::new(origin.0, origin.1))?;
    }
    // Windows 移到不同 DPI 的屏会按新 DPI 改尺寸，先移过去再定尺寸
    #[cfg(not(target_os = "macos"))]
    {
        w.set_position(tauri::PhysicalPosition::new(
            origin.0 as i32,
            origin.1 as i32,
        ))?;
        w.set_size(tauri::PhysicalSize::new(size.0 as u32, size.1 as u32))?;
    }
    Ok(())
}

/// 补齐 `shot-0` … `shot-<n-1>`；返回新建窗口的页面加载完成信号。
fn ensure_overlays(app: &AppHandle, n: usize) -> Vec<mpsc::Receiver<()>> {
    (0..n)
        .filter(|&i| app.get_webview_window(&label(i)).is_none())
        .filter_map(|i| {
            let (tx, rx) = mpsc::channel();
            match build_overlay(app, i, tx) {
                Ok(()) => Some(rx),
                Err(e) => {
                    log::warn!("创建遮罩 {i} 失败：{e}");
                    None
                }
            }
        })
        .collect()
}

fn build_overlay(app: &AppHandle, i: usize, loaded: mpsc::Sender<()>) -> tauri::Result<()> {
    let w = WebviewWindowBuilder::new(app, label(i), WebviewUrl::App("shot.html".into()))
        // 平台层 window_rects 按标题排除本进程窗口
        .title("xpaste-shot")
        .theme(crate::panel::current_theme(app))
        .visible(false)
        .focused(false)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .resizable(false)
        .skip_taskbar(true)
        .always_on_top(true)
        // 非键盘焦点屏上的第一下点击也要生效
        .accept_first_mouse(true)
        .on_page_load(move |_, p| {
            if p.event() == PageLoadEvent::Finished {
                let _ = loaded.send(());
            }
        })
        .build()?;
    #[cfg(target_os = "macos")]
    on_main(app, move |_| {
        if let Err(e) = to_overlay_panel(&w) {
            log::warn!("遮罩转换 NSPanel 失败：{e}");
        }
    });
    #[cfg(not(target_os = "macos"))]
    let _ = w;
    Ok(())
}

/// 不激活应用的 NSPanel，层级在菜单栏之上，进入任何空间与全屏应用。主线程调用；每个窗口只能调一次。
#[cfg(target_os = "macos")]
#[allow(deprecated)] // cocoa 已弃用，但 tauri-nspanel 公开的就是它的类型
fn to_overlay_panel(w: &WebviewWindow) -> tauri::Result<()> {
    use tauri_nspanel::WebviewWindowExt;
    use tauri_nspanel::cocoa::{
        appkit::{NSWindow, NSWindowCollectionBehavior as B},
        base::id,
    };
    const NS_SCREEN_SAVER_WINDOW_LEVEL: i32 = 1000;
    let mask = unsafe { (w.ns_window()? as id).styleMask() }.bits() as i32;
    let panel = w.to_panel()?;
    panel.set_style_mask(mask | crate::panel::NS_WINDOW_STYLE_MASK_NONACTIVATING_PANEL);
    panel.set_level(NS_SCREEN_SAVER_WINDOW_LEVEL);
    panel.set_collection_behaviour(
        B::NSWindowCollectionBehaviorCanJoinAllSpaces
            | B::NSWindowCollectionBehaviorFullScreenAuxiliary
            | B::NSWindowCollectionBehaviorStationary,
    );
    Ok(())
}

/// 启动时为每块显示器建好隐藏的遮罩，免得截图时现建 webview。
pub fn prewarm(app: &AppHandle) {
    let n = app.available_monitors().map_or(1, |m| m.len().max(1));
    let made = ensure_overlays(app, n).len();
    log::info!("shot 预建遮罩 {made} 个");
}

/// 遮罩画好冻结图（`shot_ready`）：上屏；全部就绪后光标所在屏拿键盘焦点。
pub fn ready(app: &AppHandle, generation: u64, i: usize) {
    let (all, key, started) = {
        let mut s = session(app);
        let Some(all) = s.mark_ready(generation, i) else {
            return;
        };
        (all, s.cursor.unwrap_or(0), s.started)
    };
    on_main(app, move |app| {
        show_overlay(app, i, false);
        if all {
            show_overlay(app, key, true);
            if timing()
                && let Some(t) = started
            {
                log::info!("shot 上屏 {}ms", t.elapsed().as_millis());
            }
        }
    });
}

/// 主线程调用。macOS 上不激活应用，前台应用保持不变。
fn show_overlay(app: &AppHandle, i: usize, focus: bool) {
    let Some(w) = app.get_webview_window(&label(i)) else {
        return;
    };
    let shown = (|| -> tauri::Result<()> {
        #[cfg(target_os = "macos")]
        {
            use tauri_nspanel::ManagerExt;
            match app.get_webview_panel(&label(i)) {
                // orderFrontRegardless + makeKeyWindow
                Ok(p) if focus => p.show(),
                Ok(p) => p.order_front_regardless(),
                Err(_) => w.show()?,
            }
            if focus {
                w.as_ref().set_focus()?;
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            w.show()?;
            if focus {
                w.set_focus()?;
            }
        }
        Ok(())
    })();
    if let Err(e) = shown {
        log::warn!("显示遮罩 {i} 失败：{e}");
    }
}

/// 隐藏全部遮罩并释放冻结图。Windows 上把键盘还给截图前的前台窗口（Esc / 右键 / 工具栏退出），
/// 不然系统按 z 序交给下一个窗口，可能是贴图；macOS 上 `previous_window` 恒为 `None`。
pub fn end(app: &AppHandle) {
    let previous = session(app).end();
    hide_overlays(app);
    if let Some(h) = previous {
        // 排在 hide_overlays 之后：遮罩还拿着前台，本进程此刻是前台进程，系统允许切
        on_main(app, move |_| xpaste_platform::activate_window(h));
    }
}

/// 隐藏全部遮罩，并撤销提示条模式的鼠标穿透，下次截图能点。
fn hide_overlays(app: &AppHandle) {
    on_main(app, |app| {
        for (l, w) in app.webview_windows() {
            if overlay_index(&l).is_none() {
                continue;
            }
            if let Err(e) = w.hide() {
                log::warn!("隐藏遮罩失败：{e}");
            }
            if let Err(e) = w.set_ignore_cursor_events(false) {
                log::warn!("撤销遮罩鼠标穿透失败：{e}");
            }
        }
    });
}

/// 遮罩的四种输出（设计 §4.5）。
#[derive(Debug, Clone, Copy)]
pub enum Action {
    Copy,
    Save,
    Ocr,
    Pin,
}

pub fn parse_action(h: &str) -> Option<Action> {
    Some(match h {
        "copy" => Action::Copy,
        "save" => Action::Save,
        "ocr" => Action::Ocr,
        "pin" => Action::Pin,
        _ => return None,
    })
}

/// `x-rect` 请求头：`x,y,w,h`，显示器内逻辑像素；宽高须为正。
pub fn parse_rect(h: &str) -> Option<LRect> {
    let v: Vec<f64> = h
        .split(',')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    match v[..] {
        [x, y, w, h] if v.iter().all(|n| n.is_finite()) && w > 0.0 && h > 0.0 => {
            Some(LRect { x, y, w, h })
        }
        _ => None,
    }
}

/// `截图 <stamp>.png`，重名依次加 ` 2`、` 3`。
pub fn shot_file_name(stamp: &str, exists: impl Fn(&str) -> bool) -> String {
    (1..)
        .map(|k| match k {
            1 => format!("截图 {stamp}.png"),
            _ => format!("截图 {stamp} {k}.png"),
        })
        .find(|n| !exists(n))
        .expect("总有不重名的序号")
}

/// 设置里截图文件夹为空时的默认：`<图片>/xPaste`，取不到图片目录时 `<数据目录>/shots`。
pub fn default_shot_dir(picture: Option<PathBuf>, data_dir: &Path) -> PathBuf {
    picture.map_or_else(|| data_dir.join("shots"), |p| p.join("xPaste"))
}

/// 设置里的截图文件夹：空或不是绝对路径（会落到进程工作目录）时用默认文件夹。
pub fn resolve_shot_dir(setting: &str, picture: Option<PathBuf>, data_dir: &Path) -> PathBuf {
    let p = Path::new(setting);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    if !setting.is_empty() {
        log::warn!("截图文件夹不是绝对路径，改用默认文件夹");
    }
    default_shot_dir(picture, data_dir)
}

/// 在 `dir` 里新建不重名的截图文件并写入，返回文件名。用 `create_new`，
/// 检查与写入之间有别人建了同名文件时换下一个序号，不会覆盖。
pub fn write_unique(dir: &Path, stamp: &str, png: &[u8]) -> std::io::Result<String> {
    use std::io::Write;
    let mut tries = 0;
    loop {
        // symlink_metadata：失效的符号链接也算占用，否则 create_new 会一直撞上它
        let name = shot_file_name(stamp, |n| dir.join(n).symlink_metadata().is_ok());
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(&name))
        {
            Ok(mut f) => {
                return match f.write_all(png) {
                    Ok(()) => Ok(name),
                    Err(e) => {
                        // 不留下写了一半的文件
                        drop(f);
                        let _ = std::fs::remove_file(dir.join(&name));
                        Err(e)
                    }
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && tries < 10 => tries += 1,
            Err(e) => return Err(e),
        }
    }
}

/// 返回给遮罩 / 贴图，用于提示条。
#[derive(Serialize, Debug)]
pub struct ShotOutcome {
    pub action: &'static str,
    /// 识别出的字数；没识别到文字为 0
    pub chars: Option<usize>,
    /// 保存的文件名（不含目录）
    pub file: Option<String>,
    /// 是否写了剪切板（保存时看“保存截图时同时复制”，识别没有文字时为 false）
    pub copied: bool,
}

/// 启动时删掉识别文字留下的 `shot-ocr-*` 临时图（屏幕像素）：正常流程用完即删，崩溃或写一半时会留下。
pub fn clear_ocr_tmp(tmp: &Path) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(tmp) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        r => r?,
    };
    for e in entries {
        let e = e?;
        if e.file_name().to_string_lossy().starts_with("shot-ocr-") {
            std::fs::remove_file(e.path())?;
        }
    }
    Ok(())
}

/// 贴图图片目录；启动时清空（贴图只在本次运行内有效）。
pub fn clear_pins(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    std::fs::create_dir_all(dir)
}

const PIN_PREFIX: &str = "pin-";
/// 贴图序号只增不复用：tauri-nspanel 按标签存指针，同名重建有重复释放的风险
static PIN_SEQ: AtomicU64 = AtomicU64::new(1);

pub fn pin_index(label: &str) -> Option<u64> {
    label.strip_prefix(PIN_PREFIX)?.parse().ok()
}

fn pin_path(app: &AppHandle, n: u64) -> PathBuf {
    app.state::<AppState>()
        .data_dir
        .join("pins")
        .join(format!("{n}.png"))
}

/// 贴图窗口的位置与尺寸（原生单位，同 `place`）：该屏 origin 加选区。
pub fn pin_bounds(
    origin: (f64, f64),
    scale: f64,
    r: LRect,
    points: bool,
) -> ((f64, f64), (f64, f64)) {
    let k = if points { 1.0 } else { scale };
    ((origin.0 + r.x * k, origin.1 + r.y * k), (r.w * k, r.h * k))
}

/// `shot_finish`：在后台线程执行。成功后遮罩进入提示条模式，前端显示提示条后调 `shot_cancel`。
pub fn finish(
    app: &AppHandle,
    action: Action,
    i: usize,
    rect: Option<LRect>,
    png: Vec<u8>,
) -> Result<ShotOutcome, AppError> {
    let (generation, origin, scale) = {
        let mut s = session(app);
        // 取到帧并认领输出要在同一次加锁里：识别文字要几秒，连按 O / 回车不能跑出两个动作
        match s.frame(i).filter(|_| s.is_active()) {
            Some(f) if s.claim_finish() => (s.generation(), f.origin, f.scale),
            _ => return Err(AppError::Invalid("截图已结束".into())),
        }
    };
    log::info!("shot 输出 {action:?} 屏 {i} {} 字节", png.len());
    match run_action(app, generation, action, origin, scale, rect, png) {
        Ok(outcome) => {
            enter_toast(app, generation);
            Ok(outcome)
        }
        Err(e) => {
            let mut s = session(app);
            if s.is_current(generation) {
                s.release_finish();
            }
            Err(e)
        }
    }
}

fn run_action(
    app: &AppHandle,
    generation: u64,
    action: Action,
    origin: (f64, f64),
    scale: f64,
    rect: Option<LRect>,
    png: Vec<u8>,
) -> Result<ShotOutcome, AppError> {
    Ok(match action {
        Action::Copy => {
            copy_png(png)?;
            ShotOutcome {
                action: "copy",
                chars: None,
                file: None,
                copied: true,
            }
        }
        Action::Save => save(app, png)?,
        Action::Ocr => ocr(app, generation, png)?,
        Action::Pin => {
            let r = rect.ok_or_else(|| AppError::Invalid("贴图缺少选区".into()))?;
            open_pin(app, origin, scale, r, &png)?;
            ShotOutcome {
                action: "pin",
                chars: None,
                file: None,
                copied: false,
            }
        }
    })
}

/// 写 PNG 到剪切板，不标记为自身写入：监听器照常把它作为图片记录入库（设计 §4.5）。
fn copy_png(png: Vec<u8>) -> Result<(), AppError> {
    let bytes = png.len();
    xpaste_platform::write(&xpaste_platform::png_flavors(png))
        .map(drop)
        .map_err(|e| {
            log::warn!("截图写剪切板失败 {bytes} 字节：{e}");
            AppError::ClipboardWrite
        })
}

/// 保存到截图文件夹；“保存截图时同时复制”开着时也写剪切板。截图与贴图共用。
fn save(app: &AppHandle, png: Vec<u8>) -> Result<ShotOutcome, AppError> {
    let state = app.state::<AppState>();
    let (dir, also_copy) = {
        let s = state.settings();
        (s.shot_dir.clone(), s.shot_save_copy)
    };
    let dir = resolve_shot_dir(&dir, app.path().picture_dir().ok(), &state.data_dir);
    std::fs::create_dir_all(&dir)?;
    let stamp = xpaste_core::capture::local_stamp(crate::clock::now_ms());
    let name = write_unique(&dir, &stamp, &png)?;
    // 文件已存好，复制失败不算整次失败（已记日志），提示条按“已保存”显示
    let copied = also_copy && copy_png(png).is_ok();
    Ok(ShotOutcome {
        action: "save",
        chars: None,
        file: Some(name),
        copied,
    })
}

/// 识别文字：PNG 写进临时文件交给系统识别；有文字就写纯文本（自身写入，不入库）。
fn ocr(app: &AppHandle, generation: u64, png: Vec<u8>) -> Result<ShotOutcome, AppError> {
    let state = app.state::<AppState>();
    let tmp = state.data_dir.join("tmp");
    std::fs::create_dir_all(&tmp)?;
    let path = tmp.join(format!("shot-ocr-{generation}.png"));
    std::fs::write(&path, &png)?;
    let t = Instant::now();
    let result = xpaste_platform::ocr_png(&path, 2000);
    let _ = std::fs::remove_file(&path);
    let text = match result {
        Ok(Some(t)) if !t.text.trim().is_empty() => Some(t.text),
        Ok(_) => None,
        Err(e) => {
            log::warn!("截图识别文字失败：{e:?}");
            return Err(AppError::Invalid("识别文字失败".into()));
        }
    };
    let chars = text.as_ref().map_or(0, |t| t.chars().count());
    log::info!("shot 识别 {chars} 字 {}ms", t.elapsed().as_millis());
    // 识别期间用户已按 Esc 关掉遮罩：不再写剪切板
    if !session(app).is_current(generation) {
        return Err(AppError::Invalid("截图已结束".into()));
    }
    if let Some(text) = &text {
        crate::commands::records::write_text_plain(&state, text)?;
    }
    Ok(ShotOutcome {
        action: "ocr",
        chars: Some(chars),
        file: None,
        copied: text.is_some(),
    })
}

/// 提示条模式：遮罩鼠标穿透、交出键盘焦点，提示条照常显示；`shot_cancel` 时撤销。
fn enter_toast(app: &AppHandle, generation: u64) {
    let previous = {
        let mut s = session(app);
        if !s.is_current(generation) {
            return;
        }
        s.finish(Instant::now())
    };
    on_main(app, move |app| {
        for (l, w) in app.webview_windows() {
            if overlay_index(&l).is_none() {
                continue;
            }
            if let Err(e) = w.set_ignore_cursor_events(true) {
                log::warn!("遮罩鼠标穿透失败：{e}");
            }
            // 键盘还给截图前的前台应用：不激活 xPaste 的 NSPanel 撤下（orderOut）时，系统把键盘交回
            // 仍处于激活状态的前台应用；随后不抢键盘地重新摆上来（orderFrontRegardless），提示条还在。
            #[cfg(target_os = "macos")]
            if w.is_focused().unwrap_or(false) {
                use tauri_nspanel::ManagerExt;
                if let Ok(p) = app.get_webview_panel(&l) {
                    p.order_out(None);
                    p.order_front_regardless();
                }
            }
        }
        // Windows：遮罩拿着前台，直接把前台切回截图前的窗口（本进程此刻是前台进程，系统允许）；
        // 记不到时等 shot_cancel 隐藏遮罩后由系统按 z 序交还
        if let Some(h) = previous {
            xpaste_platform::activate_window(h);
        }
    });
}

/// 选区原地变成贴图窗口 `pin-<n>`（设计 §4.5），图片存 `pins/<n>.png`。
fn open_pin(
    app: &AppHandle,
    origin: (f64, f64),
    scale: f64,
    r: LRect,
    png: &[u8],
) -> Result<u64, AppError> {
    let n = PIN_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = pin_path(app, n);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, png)?;
    // src：图片的 asset URL，前端（URLSearchParams）解码后直接给 <img>
    let url = format!(
        "pin.html?n={n}&w={}&h={}&src={}",
        r.w,
        r.h,
        crate::assets::encode_component(&crate::assets::asset_url(&path))
    );
    let (o, size) = pin_bounds(origin, scale, r, cfg!(target_os = "macos"));
    let w = WebviewWindowBuilder::new(app, format!("{PIN_PREFIX}{n}"), WebviewUrl::App(url.into()))
        // 平台层 window_rects 只保留本进程里标题以此开头的窗口：再次截图时贴图可被吸附
        .title("xpaste-pin")
        .theme(crate::panel::current_theme(app))
        // macOS 换成 NSPanel 后再上屏。Windows 建窗口时直接可见：tao 只在创建时按 focused(false)
        // 用 SW_SHOWNOACTIVATE，之后的 show() 一律 SW_SHOW 会激活贴图、抢走原应用的键盘；
        // 先按选区的逻辑坐标大致摆好，下面 place 再按物理像素校正
        .visible(cfg!(windows))
        .position(
            o.0 / scale_for_builder(scale),
            o.1 / scale_for_builder(scale),
        )
        .inner_size(r.w, r.h)
        .focused(false)
        .transparent(true)
        .decorations(false)
        .resizable(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .accept_first_mouse(true)
        .build()
        .inspect_err(|_| drop(std::fs::remove_file(&path)))?;
    if let Err(e) = place(&w, o, size) {
        let _ = w.destroy();
        let _ = std::fs::remove_file(&path);
        return Err(e.into());
    }
    on_main(app, move |_| show_pin(&w));
    log::info!("shot 贴图 {n}");
    Ok(n)
}

/// 主线程调用。macOS：不激活应用的 NSPanel，层级 `NSFloatingWindowLevel`，低于遮罩；点它不带出面板、不抢前台应用。
#[cfg(target_os = "macos")]
#[allow(deprecated)] // cocoa 已弃用，但 tauri-nspanel 公开的就是它的类型
fn show_pin(w: &WebviewWindow) {
    use tauri_nspanel::cocoa::{
        appkit::{NSWindow, NSWindowCollectionBehavior as B},
        base::id,
    };
    use tauri_nspanel::raw_nspanel::RawNSPanel;
    const NS_FLOATING_WINDOW_LEVEL: i32 = 3;
    let Ok(ns) = w.ns_window() else {
        log::warn!("贴图取不到 NSWindow");
        return;
    };
    let mask = unsafe { (ns as id).styleMask() }.bits() as i32;
    // 不用 to_panel：插件会按标签一直持有指针，贴图销毁后那条记录悬空。from_window 只换类、不登记；
    // 它返回的 Id 没有自己的引用计数（from_retained_ptr），所以不能 drop
    let panel = std::mem::ManuallyDrop::new(RawNSPanel::from_window(w.clone()));
    panel.set_style_mask(mask | crate::panel::NS_WINDOW_STYLE_MASK_NONACTIVATING_PANEL);
    panel.set_level(NS_FLOATING_WINDOW_LEVEL);
    panel.set_collection_behaviour(
        B::NSWindowCollectionBehaviorCanJoinAllSpaces
            | B::NSWindowCollectionBehaviorFullScreenAuxiliary,
    );
    panel.order_front_regardless();
}

/// Windows 的贴图建窗口时已不激活地上屏，这里不再 show（会激活它）。
#[cfg(not(target_os = "macos"))]
fn show_pin(_w: &WebviewWindow) {}

/// builder 的 position 是逻辑坐标；macOS 的原生单位已是点，Windows 的是物理像素，要除以该屏缩放。
fn scale_for_builder(scale: f64) -> f64 {
    if cfg!(target_os = "macos") {
        1.0
    } else {
        scale
    }
}

fn pin_n(w: &WebviewWindow) -> Result<u64, AppError> {
    pin_index(w.label()).ok_or_else(|| AppError::Invalid("不是贴图窗口".into()))
}

/// 关闭贴图：删图片、销毁窗口。
pub fn pin_close(app: &AppHandle, w: &WebviewWindow) -> Result<(), AppError> {
    let n = pin_n(w)?;
    match std::fs::remove_file(pin_path(app, n)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            log::warn!("删除贴图 {n} 图片失败：{e}")
        }
        _ => {}
    }
    log::info!("shot 关闭贴图 {n}");
    w.destroy()?;
    Ok(())
}

const MAX_PIN_PX: f64 = 16384.0;

/// 贴图滚轮缩放、恢复原始大小、拖回屏内：逻辑坐标，位置和尺寸一起改。同步命令跑在主线程，
/// `place` 的两步在同一轮里改完、中间不重绘，以光标为锚的缩放不会先跳一下。
pub fn pin_set_bounds(
    w: &WebviewWindow,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), AppError> {
    pin_n(w)?;
    if ![x, y, width, height].iter().all(|v| v.is_finite()) || width < 1.0 || height < 1.0 {
        return Err(AppError::Invalid("贴图尺寸无效".into()));
    }
    let s = w.scale_factor()?;
    // 整屏贴图放大到 400% 会超出 webview 的纹理上限：等比缩到每边不超过 16384 物理像素
    let f = (MAX_PIN_PX / s / width.max(height)).min(1.0);
    let (width, height) = (width * f, height * f);
    // 换成原生单位：macOS 为点，Windows 为物理像素（同 builder 的换算）
    let k = scale_for_builder(s);
    place(w, (x * k, y * k), (width * k, height * k))?;
    Ok(())
}

/// 贴图右键“复制”：原图写剪切板，进入历史（同截图的复制）。
pub fn pin_copy(app: &AppHandle, w: &WebviewWindow) -> Result<ShotOutcome, AppError> {
    copy_png(std::fs::read(pin_path(app, pin_n(w)?))?)?;
    Ok(ShotOutcome {
        action: "copy",
        chars: None,
        file: None,
        copied: true,
    })
}

/// 贴图右键“保存”：原图，同截图的保存规则。
pub fn pin_save(app: &AppHandle, w: &WebviewWindow) -> Result<ShotOutcome, AppError> {
    save(app, std::fs::read(pin_path(app, pin_n(w)?))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use xpaste_platform::{Frame, WinRect};

    #[test]
    fn screen_capture_pane_renamed_since_macos_15() {
        assert_eq!(screen_capture_pane(Some(15)), "录屏与系统录音");
        assert_eq!(screen_capture_pane(Some(26)), "录屏与系统录音");
        assert_eq!(screen_capture_pane(Some(14)), "屏幕录制");
        assert_eq!(screen_capture_pane(None), "屏幕录制");
    }

    fn frame(origin: (f64, f64), size: (f64, f64), scale: f64) -> Frame {
        Frame {
            origin,
            size,
            scale,
            px: (0, 0),
            rgba: Vec::new(),
        }
    }

    #[test]
    fn to_local_mixed_scale_macos_points() {
        let main = frame((0.0, 0.0), (1512.0, 982.0), 2.0);
        let ext = frame((1512.0, 0.0), (1920.0, 1080.0), 1.0);
        let w = WinRect {
            x: 1600.0,
            y: 100.0,
            w: 800.0,
            h: 600.0,
        };
        assert_eq!(
            to_local(&w, &ext, true),
            Some(LRect {
                x: 88.0,
                y: 100.0,
                w: 800.0,
                h: 600.0
            })
        );
        assert_eq!(to_local(&w, &main, true), None);
    }

    #[test]
    fn to_local_windows_physical() {
        let f = frame((-2880.0, 0.0), (2880.0, 1800.0), 1.5);
        let w = WinRect {
            x: -2730.0,
            y: 150.0,
            w: 1500.0,
            h: 900.0,
        };
        assert_eq!(
            to_local(&w, &f, false),
            Some(LRect {
                x: 100.0,
                y: 100.0,
                w: 1000.0,
                h: 600.0
            })
        );
        // 跨到右边主屏的窗口：只留左屏内的 300 物理像素
        let across = WinRect {
            x: -300.0,
            y: 0.0,
            w: 900.0,
            h: 300.0,
        };
        assert_eq!(
            to_local(&across, &f, false),
            Some(LRect {
                x: 1720.0,
                y: 0.0,
                w: 200.0,
                h: 200.0
            })
        );
    }

    #[test]
    fn cursor_hits_screen_in_native_units() {
        let frames = [
            frame((0.0, 0.0), (1512.0, 982.0), 2.0),
            frame((1512.0, 0.0), (1920.0, 1080.0), 1.0),
        ];
        assert_eq!(
            cursor_on(&frames, (1600.0, 50.0), true),
            Some((1, (88.0, 50.0)))
        );
        assert_eq!(cursor_on(&frames, (-1.0, 50.0), true), None);
        let win = [frame((-2880.0, 0.0), (2880.0, 1800.0), 1.5)];
        assert_eq!(
            cursor_on(&win, (-2730.0, 150.0), false),
            Some((0, (100.0, 100.0)))
        );
    }

    #[test]
    fn begin_plan_needs_permission() {
        assert!(matches!(begin_plan(false), BeginPlan::NeedPermission));
        assert!(matches!(begin_plan(true), BeginPlan::Freeze));
    }

    #[test]
    fn second_begin_is_ignored_until_end() {
        let mut s = Session::default();
        assert!(s.try_begin());
        assert!(!s.try_begin());
        assert!(s.is_active());
        s.end();
        assert!(!s.is_active());
        assert!(s.try_begin());
    }

    #[test]
    fn end_drops_frames() {
        let mut s = Session::default();
        s.try_begin();
        s.set_frames(vec![
            frame((0.0, 0.0), (10.0, 10.0), 1.0),
            frame((10.0, 0.0), (10.0, 10.0), 1.0),
        ]);
        assert!(s.frame(1).is_some());
        s.end();
        assert!(s.frame(0).is_none());
    }

    #[test]
    fn all_ready_fires_once() {
        let mut s = Session::default();
        s.try_begin();
        s.set_frames(vec![
            frame((0.0, 0.0), (10.0, 10.0), 1.0),
            frame((10.0, 0.0), (10.0, 10.0), 1.0),
        ]);
        let g = s.generation();
        assert_eq!(s.mark_ready(g, 1), Some(false));
        assert_eq!(s.mark_ready(g, 1), Some(false), "重复就绪不算数");
        assert_eq!(s.mark_ready(g, 5), Some(false), "越界序号忽略");
        assert_eq!(s.mark_ready(g, 0), Some(true));
        assert_eq!(s.mark_ready(g, 0), Some(false));
    }

    #[test]
    fn stale_session_is_replaced_after_3s() {
        let t0 = Instant::now();
        let mut s = Session::default();
        assert!(s.try_begin_at(t0));
        let g1 = s.generation();
        s.set_frames(vec![
            frame((0.0, 0.0), (10.0, 10.0), 1.0),
            frame((10.0, 0.0), (10.0, 10.0), 1.0),
        ]);
        s.mark_ready(g1, 0);
        assert!(!s.try_begin_at(t0 + Duration::from_secs(2)));
        assert!(!s.try_begin_at(t0 + STALE), "正好 3 s 还不算卡住");
        assert!(s.try_begin_at(t0 + Duration::from_millis(3001)));
        assert!(s.frame(0).is_none(), "旧冻结图已释放");
        assert!(s.generation() > g1);
        // 冻结还没回来（尚无帧）也算未就绪
        assert!(s.try_begin_at(t0 + Duration::from_secs(7)));
    }

    #[test]
    fn all_ready_session_is_never_stale() {
        let t0 = Instant::now();
        let mut s = Session::default();
        s.try_begin_at(t0);
        s.set_frames(vec![frame((0.0, 0.0), (10.0, 10.0), 1.0)]);
        s.mark_ready(s.generation(), 0);
        assert!(!s.try_begin_at(t0 + Duration::from_secs(60)));
    }

    #[test]
    fn old_freeze_generation_is_not_current() {
        let mut s = Session::default();
        s.try_begin();
        let g1 = s.generation();
        assert!(s.is_current(g1));
        s.end();
        assert!(!s.is_current(g1));
        s.try_begin();
        assert!(!s.is_current(g1), "begin → end → begin 后旧线程不能写入");
        assert!(s.is_current(s.generation()));
    }

    #[test]
    fn payload_until_ready() {
        let mut s = Session::default();
        s.try_begin();
        assert_eq!(s.payload(0), None, "冻结完成前没有载荷");
        s.set_frames(vec![
            frame((0.0, 0.0), (10.0, 10.0), 1.0),
            frame((10.0, 0.0), (10.0, 10.0), 2.0),
        ]);
        let generation = s.generation();
        let p = |index, scale| BeginPayload {
            index,
            generation,
            rects: vec![],
            cursor: None,
            scale,
            save_copy: true,
        };
        s.set_payloads(vec![p(0, 1.0), p(1, 2.0)]);
        assert_eq!(s.payload(1), Some(p(1, 2.0)));
        s.mark_ready(generation, 1);
        assert_eq!(s.payload(1), None, "已上屏不再给");
        assert_eq!(s.payload(0), Some(p(0, 1.0)));
        s.end();
        assert_eq!(s.payload(0), None);
    }

    #[test]
    fn frame_bytes_prefixes_le_size() {
        let mut f = frame((0.0, 0.0), (1.0, 1.0), 1.0);
        f.px = (258, 1);
        f.rgba = vec![9; 258 * 4];
        let b = frame_bytes(&f);
        assert_eq!(&b[..8], &[2, 1, 0, 0, 1, 0, 0, 0]);
        assert_eq!(b.len(), 8 + 258 * 4);
    }

    #[test]
    fn shot_file_name_appends_counter() {
        let taken = ["截图 2026-10-28 14.32.png", "截图 2026-10-28 14.32 2.png"];
        assert_eq!(
            shot_file_name("2026-10-28 14.32", |n| taken.contains(&n)),
            "截图 2026-10-28 14.32 3.png"
        );
        assert_eq!(
            shot_file_name("2026-10-28 14.32", |_| false),
            "截图 2026-10-28 14.32.png"
        );
    }

    #[test]
    fn parse_action_rejects_unknown() {
        assert!(matches!(parse_action("copy"), Some(Action::Copy)));
        assert!(matches!(parse_action("pin"), Some(Action::Pin)));
        assert!(parse_action("rm").is_none());
    }

    #[test]
    fn parse_rect_needs_four_numbers() {
        assert_eq!(
            parse_rect("10,20.5,300,200"),
            Some(LRect {
                x: 10.0,
                y: 20.5,
                w: 300.0,
                h: 200.0
            })
        );
        assert_eq!(parse_rect("1,2,3"), None);
        assert_eq!(parse_rect("1,2,3,x"), None);
        assert_eq!(parse_rect("1,2,0,4"), None, "空选区");
    }

    #[test]
    fn default_shot_dir_falls_back() {
        assert_eq!(
            default_shot_dir(Some("/p".into()), Path::new("/d")),
            PathBuf::from("/p/xPaste")
        );
        assert_eq!(
            default_shot_dir(None, Path::new("/d")),
            PathBuf::from("/d/shots")
        );
    }

    #[test]
    fn pins_dir_cleared_on_start() {
        let dir = std::env::temp_dir().join(format!("xpaste-pins-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("1.png"), b"a").unwrap();
        std::fs::write(dir.join("2.png"), b"b").unwrap();
        clear_pins(&dir).unwrap();
        assert!(dir.is_dir());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn ocr_tmp_cleared_on_start() {
        let dir = std::env::temp_dir().join(format!("xpaste-ocr-tmp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shot-ocr-3.png"), b"a").unwrap();
        std::fs::write(dir.join("shot-ocr-4.png"), b"").unwrap();
        std::fs::write(dir.join("other.png"), b"b").unwrap();
        clear_ocr_tmp(&dir).unwrap();
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["other.png"], "只删截图识别的临时图");
        std::fs::remove_dir_all(&dir).unwrap();
        // 目录不存在不算错
        clear_ocr_tmp(&dir).unwrap();
    }

    #[test]
    fn previous_window_handed_back_once() {
        let mut s = Session::default();
        s.try_begin();
        s.previous_window = Some(7);
        // 动作完成（提示条）时交还；之后 shot_cancel 结束会话时不再切回
        assert_eq!(s.finish(Instant::now()), Some(7));
        assert_eq!(s.end(), None);
        // Esc 直接退出：结束会话时交还
        s.try_begin();
        s.previous_window = Some(9);
        assert_eq!(s.end(), Some(9));
        assert_eq!(s.end(), None);
    }

    #[test]
    fn pin_bounds_in_native_units() {
        let r = LRect {
            x: 10.0,
            y: 20.0,
            w: 300.0,
            h: 200.0,
        };
        // macOS：原生单位就是点
        assert_eq!(
            pin_bounds((1512.0, 0.0), 2.0, r, true),
            ((1522.0, 20.0), (300.0, 200.0))
        );
        // Windows：物理像素
        assert_eq!(
            pin_bounds((-2880.0, 0.0), 1.5, r, false),
            ((-2865.0, 30.0), (450.0, 300.0))
        );
    }

    #[test]
    fn pin_index_from_label() {
        assert_eq!(pin_index("pin-12"), Some(12));
        assert_eq!(pin_index("shot-1"), None);
    }

    #[test]
    fn stale_generation_ready_is_ignored() {
        let mut s = Session::default();
        s.try_begin();
        let g1 = s.generation();
        s.end();
        s.try_begin();
        s.set_frames(vec![frame((0.0, 0.0), (10.0, 10.0), 1.0)]);
        assert_eq!(s.mark_ready(g1, 0), None, "上一次截图的遮罩迟到的就绪不算");
        assert_eq!(s.payload(0).map(|p| p.index), None, "还没有载荷");
        assert_eq!(s.mark_ready(s.generation(), 0), Some(true));
    }

    #[test]
    fn finish_drops_frames_and_blocks_begin_until_stale() {
        let t0 = Instant::now();
        let mut s = Session::default();
        s.try_begin_at(t0);
        let g = s.generation();
        s.set_frames(vec![frame((0.0, 0.0), (10.0, 10.0), 1.0)]);
        s.mark_ready(g, 0);
        let t1 = t0 + Duration::from_secs(10);
        s.finish(t1);
        assert!(s.frame(0).is_none(), "动作完成即释放冻结图");
        assert!(s.is_current(g), "提示条期间会话仍在");
        assert!(
            !s.try_begin_at(t1 + Duration::from_secs(1)),
            "提示条期间按快捷键忽略"
        );
        // 前端没调 shot_cancel 时，下次快捷键照常开始
        assert!(s.try_begin_at(t1 + Duration::from_millis(3001)));
    }

    #[test]
    fn second_finish_is_refused_until_released() {
        let mut s = Session::default();
        s.try_begin();
        assert!(s.claim_finish());
        assert!(!s.claim_finish(), "识别文字还在跑，第二个动作被拒绝");
        s.release_finish();
        assert!(s.claim_finish(), "上一个动作失败后可以重试");
        s.finish(Instant::now());
        s.end();
        s.try_begin();
        assert!(s.claim_finish(), "新的一次截图重新可认领");
    }

    #[test]
    fn parse_rect_rejects_non_finite() {
        assert_eq!(parse_rect("NaN,0,10,10"), None);
        assert_eq!(parse_rect("0,0,inf,10"), None);
    }

    #[test]
    fn relative_shot_dir_falls_back_to_default() {
        let d = Path::new("/d");
        // Windows 上 `/mine` 不是绝对路径
        let mine = if cfg!(windows) { r"C:\mine" } else { "/mine" };
        assert_eq!(resolve_shot_dir(mine, None, d), PathBuf::from(mine));
        assert_eq!(resolve_shot_dir("", None, d), PathBuf::from("/d/shots"));
        assert_eq!(
            resolve_shot_dir("shots", Some("/p".into()), d),
            PathBuf::from("/p/xPaste")
        );
    }

    #[test]
    fn write_unique_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("xpaste-shot-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("截图 s.png"), b"old").unwrap();
        assert_eq!(write_unique(&dir, "s", b"new").unwrap(), "截图 s 2.png");
        assert_eq!(std::fs::read(dir.join("截图 s.png")).unwrap(), b"old");
        assert_eq!(std::fs::read(dir.join("截图 s 2.png")).unwrap(), b"new");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn overlay_index_from_label() {
        assert_eq!(overlay_index("shot-2"), Some(2));
        assert_eq!(overlay_index("panel"), None);
        assert_eq!(overlay_index("shot-x"), None);
    }
}
