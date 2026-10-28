//! 截图会话与每屏遮罩窗口（设计 §4.1、§4.3）。
//!
//! 流程：快捷键 → `begin` 在后台线程冻结所有显示器 → 给每块屏的遮罩 `shot-<i>` 发 `shot-begin`
//! → 遮罩用 `shot_frame` 取冻结图、画好后 `shot_ready` 上屏 → 全部就绪后光标所在屏拿键盘焦点。

use std::sync::{Arc, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use xpaste_platform::{Frame, WinRect};

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

    /// 结束并释放冻结图（每屏几十 MB）；代数保留。
    pub fn end(&mut self) {
        *self = Self {
            generation: self.generation,
            ..Self::default()
        };
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    fn all_ready(&self) -> bool {
        !self.ready.is_empty() && self.ready.iter().all(|r| *r)
    }

    /// 记第 `i` 块屏已上屏；恰好让全部就绪的那一次返回 true，重复或越界的序号不算。
    pub fn mark_ready(&mut self, i: usize) -> bool {
        match self.ready.get_mut(i) {
            Some(r) if !*r => {
                *r = true;
                self.ready.iter().all(|r| *r)
            }
            _ => false,
        }
    }
}

/// `shot-begin` 的载荷，前端 `src/api/shot.ts` 与之对应。
#[derive(Serialize, Clone, PartialEq, Debug)]
pub struct BeginPayload {
    index: usize,
    /// 与该屏相交的窗口，由前到后，供吸附
    rects: Vec<LRect>,
    /// 只有光标所在那块屏有值
    cursor: Option<(f64, f64)>,
    scale: f64,
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

/// 快捷键入口。主线程调用。
#[allow(dead_code)] // Task 7 接快捷键
pub fn begin(app: &AppHandle) {
    if let BeginPlan::NeedPermission = begin_plan(xpaste_platform::screen_capture_granted()) {
        log::info!("截图缺少屏幕录制权限，提示授权");
        crate::panel::show(app, None, crate::panel::Trigger::Hotkey);
        events::emit_to(app, "panel", events::SHOT_NEED_PERMISSION, ());
        return;
    }
    let (generation, stale) = {
        let mut s = session(app);
        let was_active = s.is_active();
        if !s.try_begin() {
            return;
        }
        (s.generation(), was_active)
    };
    // 上一次卡住的会话：已上屏的遮罩还盖在屏上，先撤下，不然会被截进去
    if stale {
        log::info!("shot 上次截图 {}s 内未全部上屏，重新开始", STALE.as_secs());
        hide_overlays(app);
    }
    // 面板不能被截进去
    let panel_shown = app
        .get_webview_window("panel")
        .is_some_and(|w| w.is_visible().unwrap_or(false));
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
    let payloads: Vec<BeginPayload> = frames
        .iter()
        .enumerate()
        .map(|(i, f)| BeginPayload {
            index: i,
            rects: rects
                .iter()
                .filter_map(|r| to_local(r, f, points))
                .collect(),
            cursor: cursor.filter(|(j, _)| *j == i).map(|(_, p)| p),
            scale: f.scale,
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
    // 进程里第一次冻结要多花几十毫秒，启动时空转一次；没权限时不调，免得启动就弹授权
    if xpaste_platform::screen_capture_granted() {
        std::thread::spawn(|| drop(xpaste_platform::freeze_displays()));
    }
}

/// 遮罩画好冻结图（`shot_ready`）：上屏；全部就绪后光标所在屏拿键盘焦点。
pub fn ready(app: &AppHandle, i: usize) {
    let (all, key, started) = {
        let mut s = session(app);
        if !s.is_active() {
            return;
        }
        (s.mark_ready(i), s.cursor.unwrap_or(0), s.started)
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

/// 隐藏全部遮罩并释放冻结图。
pub fn end(app: &AppHandle) {
    session(app).end();
    hide_overlays(app);
}

fn hide_overlays(app: &AppHandle) {
    on_main(app, |app| {
        for (l, w) in app.webview_windows() {
            if overlay_index(&l).is_some()
                && let Err(e) = w.hide()
            {
                log::warn!("隐藏遮罩失败：{e}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use xpaste_platform::{Frame, WinRect};

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
        assert!(!s.mark_ready(1));
        assert!(!s.mark_ready(1), "重复就绪不算数");
        assert!(!s.mark_ready(5), "越界序号忽略");
        assert!(s.mark_ready(0));
        assert!(!s.mark_ready(0));
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
        s.mark_ready(0);
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
        s.mark_ready(0);
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
        let p = |index, scale| BeginPayload {
            index,
            rects: vec![],
            cursor: None,
            scale,
        };
        s.set_payloads(vec![p(0, 1.0), p(1, 2.0)]);
        assert_eq!(s.payload(1), Some(p(1, 2.0)));
        s.mark_ready(1);
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
    fn overlay_index_from_label() {
        assert_eq!(overlay_index("shot-2"), Some(2));
        assert_eq!(overlay_index("panel"), None);
        assert_eq!(overlay_index("shot-x"), None);
    }
}
