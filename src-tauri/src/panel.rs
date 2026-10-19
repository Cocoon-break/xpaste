use std::sync::OnceLock;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Monitor, PhysicalPosition,
    PhysicalSize, Theme, WebviewWindow,
};
use xpaste_core::Appearance;

use crate::state::AppState;

const LABEL: &str = "panel";

/// 面板材质：系统毛玻璃生效时为 `"glass"`，否则 `"opaque"`。
static MATERIAL: OnceLock<&'static str> = OnceLock::new();

pub fn material() -> &'static str {
    MATERIAL.get().copied().unwrap_or("opaque")
}

/// 无 Dock 图标；把面板窗口换成不激活应用的 NSPanel，浮在全屏应用之上；铺系统毛玻璃。
/// 只有激活策略失败才返回错误，NSPanel / 毛玻璃失败只记日志。
#[cfg(target_os = "macos")]
#[allow(deprecated)] // cocoa 已弃用，但 tauri-nspanel 公开的就是它的类型
pub fn setup_macos(app: &tauri::App) -> Result<(), crate::error::AppError> {
    use tauri_nspanel::WebviewWindowExt;
    use tauri_nspanel::cocoa::{
        appkit::{NSWindow, NSWindowCollectionBehavior as B},
        base::id,
    };
    use window_vibrancy::{NSVisualEffectMaterial, NSVisualEffectState};
    // AppKit 常量，cocoa 未导出
    const NS_WINDOW_STYLE_MASK_NONACTIVATING_PANEL: i32 = 1 << 7;
    const NS_FLOATING_WINDOW_LEVEL: i32 = 3;

    app.handle()
        .set_activation_policy(tauri::ActivationPolicy::Accessory)?;
    let Some(w) = window(app.handle()) else {
        return Ok(());
    };
    let to_panel = || -> tauri::Result<()> {
        let mask = unsafe { (w.ns_window()? as id).styleMask() }.bits() as i32;
        // to_panel 只能调一次；setup 只跑一次
        let panel = w.to_panel()?;
        panel.set_style_mask(mask | NS_WINDOW_STYLE_MASK_NONACTIVATING_PANEL);
        panel.set_level(NS_FLOATING_WINDOW_LEVEL);
        panel.set_collection_behaviour(
            B::NSWindowCollectionBehaviorCanJoinAllSpaces
                | B::NSWindowCollectionBehaviorFullScreenAuxiliary,
        );
        Ok(())
    };
    if let Err(e) = to_panel() {
        log::warn!("转换 NSPanel 失败：{e}");
    }
    match window_vibrancy::apply_vibrancy(
        &w,
        NSVisualEffectMaterial::HudWindow,
        Some(NSVisualEffectState::Active),
        Some(16.0),
    ) {
        Ok(()) => {
            let _ = MATERIAL.set("glass");
        }
        Err(e) => log::warn!("毛玻璃不可用：{e}"),
    }
    Ok(())
}

/// Windows 材质选择：Mica 成功即毛玻璃；Windows 10 不支持 Mica 时直接不透明
/// （Acrylic 在 Windows 10 上拖动缩放会卡顿）；其他错误再试 Acrylic，仍失败则不透明。
#[cfg_attr(not(windows), allow(dead_code))]
pub fn windows_material(
    mica: Result<(), window_vibrancy::Error>,
    acrylic: impl FnOnce() -> Result<(), window_vibrancy::Error>,
) -> &'static str {
    match mica {
        Ok(()) => "glass",
        Err(window_vibrancy::Error::UnsupportedPlatformVersion(_)) => "opaque",
        Err(e) => {
            log::debug!("Mica 不可用：{e}");
            match acrylic() {
                Ok(()) => "glass",
                Err(e) => {
                    log::debug!("Acrylic 不可用：{e}");
                    "opaque"
                }
            }
        }
    }
}

/// 铺 Windows 系统材质。`dark: None` 跟随窗口主题，`apply_theme` 的 `set_theme` 会同步 Mica 明暗。
#[cfg(windows)]
pub fn setup_windows(app: &tauri::App) {
    use window_vibrancy::{apply_acrylic, apply_mica};
    let Some(w) = window(app.handle()) else {
        return;
    };
    let m = windows_material(apply_mica(&w, None), || apply_acrylic(&w, None));
    if m == "opaque" {
        log::info!("系统材质不可用，使用不透明背景");
    }
    let _ = MATERIAL.set(m);
}

/// 逻辑像素的矩形区域。
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 窗口左上角：水平居中、顶边在工作区 1/6 处，并钳制在工作区内。
pub fn place_in_work_area(work: Rect, (w, h): (f64, f64)) -> (f64, f64) {
    // 先 min 后 max（不用 f64::clamp）：窗口比工作区大时贴左上而不是 panic
    let x = (work.x + (work.w - w) / 2.0)
        .min(work.x + work.w - w)
        .max(work.x);
    let y = (work.y + work.h / 6.0).min(work.y + work.h - h).max(work.y);
    (x, y)
}

fn window(app: &AppHandle) -> Option<WebviewWindow> {
    let w = app.get_webview_window(LABEL);
    if w.is_none() {
        log::warn!("找不到面板窗口");
    }
    w
}

/// 光标是否落在显示器内。macOS 上 tao 的光标按主屏缩放、显示器按各自缩放换算成物理像素，
/// 两者不在同一空间，所以都换回点（`points_space`）再比；其他平台物理对物理。
fn monitor_contains(
    cursor_phys: (f64, f64),
    primary_sf: f64,
    pos_phys: (f64, f64),
    size_phys: (f64, f64),
    sf: f64,
    points_space: bool,
) -> bool {
    let (cs, ms) = if points_space {
        (primary_sf, sf)
    } else {
        (1.0, 1.0)
    };
    let (cx, cy) = (cursor_phys.0 / cs, cursor_phys.1 / cs);
    let (x0, y0) = (pos_phys.0 / ms, pos_phys.1 / ms);
    let (x1, y1) = (
        (pos_phys.0 + size_phys.0) / ms,
        (pos_phys.1 + size_phys.1) / ms,
    );
    cx >= x0 && cx < x1 && cy >= y0 && cy < y1
}

/// 鼠标所在显示器；找不到时用主显示器。
fn cursor_monitor(app: &AppHandle) -> tauri::Result<Option<Monitor>> {
    let c = app.cursor_position()?;
    let primary = app.primary_monitor()?;
    let primary_sf = primary.as_ref().map_or(1.0, |m| m.scale_factor());
    let hit = app.available_monitors()?.into_iter().find(|m| {
        let (p, s) = (m.position(), m.size());
        monitor_contains(
            (c.x, c.y),
            primary_sf,
            (p.x as f64, p.y as f64),
            (s.width as f64, s.height as f64),
            m.scale_factor(),
            cfg!(target_os = "macos"),
        )
    });
    Ok(hit.or(primary))
}

/// 移到鼠标所在屏的工作区；失败只记日志，不影响显示。
fn place(app: &AppHandle, w: &WebviewWindow) -> tauri::Result<()> {
    let size = app
        .state::<AppState>()
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .panel_size;
    let Some(m) = cursor_monitor(app)? else {
        return Ok(());
    };
    let (sf, wa) = (m.scale_factor(), m.work_area());
    let work = Rect {
        x: wa.position.x as f64 / sf,
        y: wa.position.y as f64 / sf,
        w: wa.size.width as f64 / sf,
        h: wa.size.height as f64 / sf,
    };
    let (x, y) = place_in_work_area(work, size);
    if cfg!(target_os = "macos") {
        // macOS 的逻辑坐标就是点，与显示器缩放无关
        w.set_size(LogicalSize::new(size.0, size.1))?;
        w.set_position(LogicalPosition::new(x, y))
    } else {
        // tao 把 Logical* 按窗口“当前”所在显示器的缩放换回物理像素，而上面按“目标”显示器换算；
        // 混合 DPI（如主屏 150%、外接 100%）下会落到屏外，所以这里自己按目标缩放换算后传物理值。
        // 先移后定尺寸：跨缩放比例移动会触发 WM_DPICHANGED，tao 按系统建议矩形缩放窗口，最终尺寸以这里为准
        w.set_position(PhysicalPosition::new(
            (x * sf).round() as i32,
            (y * sf).round() as i32,
        ))?;
        w.set_size(PhysicalSize::new(
            (size.0 * sf).round() as u32,
            (size.1 * sf).round() as u32,
        ))
    }
}

fn try_show(app: &AppHandle, w: &WebviewWindow, t0: Option<u64>) -> tauri::Result<()> {
    if let Err(e) = place(app, w) {
        log::warn!("面板定位失败：{e}");
    }
    #[cfg(target_os = "macos")]
    {
        use tauri_nspanel::ManagerExt;
        match app.get_webview_panel(LABEL) {
            // orderFrontRegardless + makeKeyWindow，不激活应用，前台应用保持不变
            Ok(p) => p.show(),
            Err(_) => w.show()?,
        }
        // 键盘交给 WKWebView（只设 first responder，不激活应用）
        w.as_ref().set_focus()?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        w.show()?;
        w.set_focus()?;
    }
    w.emit(crate::events::PANEL_SHOWN, serde_json::json!({ "t0": t0 }))
}

/// AppKit 只能在主线程调用；已在主线程时同步执行（hide_panel 命令跑在异步线程上）。
fn on_main(app: &AppHandle, f: impl FnOnce(&AppHandle) + Send + 'static) {
    let a = app.clone();
    if let Err(e) = app.run_on_main_thread(move || f(&a)) {
        log::warn!("切换到主线程失败：{e}");
    }
}

pub fn show(app: &AppHandle, t0: Option<u64>) {
    app.state::<AppState>().previous_window.store(
        xpaste_platform::foreground_window().unwrap_or(0),
        std::sync::atomic::Ordering::Relaxed,
    );
    show_panel(app, t0);
}

/// 粘贴失败后重新显示面板给出降级提示；不覆盖 `previous_window`（此时前台可能已是别的窗口）。
pub fn reshow(app: &AppHandle) {
    show_panel(app, None);
}

fn show_panel(app: &AppHandle, t0: Option<u64>) {
    HIDE_REQ.store(0, Ordering::Relaxed);
    on_main(app, move |app| {
        if let Some(w) = window(app)
            && let Err(e) = try_show(app, &w, t0)
        {
            log::warn!("显示面板失败：{e}");
        }
    });
}

pub fn hide(app: &AppHandle) {
    HIDE_REQ.store(0, Ordering::Relaxed);
    on_main(app, |app| {
        // NSPanel 上 hide 即 orderOut:
        let Some(w) = window(app) else { return };
        match w.hide() {
            Ok(()) => {
                LAST_HIDE_MS.store(crate::clock::now_ms(), Ordering::Relaxed);
                crate::events::emit(app, crate::events::PANEL_HIDDEN, ())
            }
            Err(e) => log::warn!("隐藏面板失败：{e}"),
        }
    });
}

/// 待处理的隐藏请求编号，0 表示没有；`show_panel` / `hide` 时清零。
static HIDE_REQ: AtomicU64 = AtomicU64::new(0);
static REQ_SEQ: AtomicU64 = AtomicU64::new(1);
/// 前端淡出 160 ms 后调 `hide_panel`；超过这个时间窗口仍可见，说明前端没响应。
const HIDE_FALLBACK: Duration = Duration::from_millis(400);

/// 最近一次成功隐藏的时间（Unix 毫秒）；`i64::MIN` 表示从未隐藏。
static LAST_HIDE_MS: AtomicI64 = AtomicI64::new(i64::MIN);
/// 隐藏后这段时间内的托盘单击不再唤起：这次按下已让面板失焦隐藏。
pub const TRAY_AFTER_HIDE_MS: u64 = 300;

/// `toggle` 的来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Hotkey,
    Tray,
    /// 再次启动（第二实例 / macOS 重开 .app）
    Relaunch,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ToggleAction {
    /// `then_hotkey`：显示后再通知前端“快捷键已按下”（引导中按快捷键唤起）
    Show {
        then_hotkey: bool,
    },
    /// 淡出途中重新唤起，不重记目标窗口（此时前台是面板自己）
    Reshow,
    RequestHide,
    /// 引导中面板不隐藏，通知前端“快捷键已按下”
    HotkeyPressed,
    Ignore,
}

/// 淡出途中再按一次视为重新唤起；引导中可见时快捷键不隐藏，托盘与再次启动不算试按；
/// 托盘单击刚让面板失焦隐藏时不再唤起。
pub fn toggle_action(
    visible: bool,
    hide_pending: bool,
    onboarding: bool,
    trigger: Trigger,
    ms_since_hide: u64,
) -> ToggleAction {
    if visible && hide_pending {
        ToggleAction::Reshow
    } else if visible && onboarding && trigger == Trigger::Hotkey {
        ToggleAction::HotkeyPressed
    } else if visible {
        ToggleAction::RequestHide
    } else if trigger == Trigger::Tray && ms_since_hide < TRAY_AFTER_HIDE_MS {
        ToggleAction::Ignore
    } else {
        ToggleAction::Show {
            then_hotkey: onboarding && trigger == Trigger::Hotkey,
        }
    }
}

/// 以窗口实际可见性为准切换。可见时不直接隐藏，请前端先播淡出再调 `hide_panel`；
/// 前端失效时由兜底线程隐藏。
pub fn toggle(app: &AppHandle, t0: Option<u64>, trigger: Trigger) {
    let Some(w) = window(app) else { return };
    let visible = match w.is_visible() {
        Ok(v) => v,
        Err(e) => return log::warn!("读取面板可见性失败：{e}"),
    };
    let onboarding = app.state::<AppState>().onboarding.load(Ordering::SeqCst);
    // 时钟回拨（差为负）当作很久以前隐藏，宁可漏挡一次也不让托盘失灵
    let since = crate::clock::now_ms().saturating_sub(LAST_HIDE_MS.load(Ordering::Relaxed));
    let since = u64::try_from(since).unwrap_or(u64::MAX);
    let hide_pending = HIDE_REQ.load(Ordering::Relaxed) != 0;
    match toggle_action(visible, hide_pending, onboarding, trigger, since) {
        ToggleAction::Show { then_hotkey } => {
            show(app, t0);
            // 引导中先切到别的应用（面板失焦隐藏）再按快捷键，也算试按成功。
            // 排在 show 的主线程任务之后发，前端先收到 panel-shown、重新打开引导，再收到这个
            if then_hotkey {
                on_main(app, |app| {
                    crate::events::emit(app, crate::events::HOTKEY_PRESSED, ())
                });
            }
        }
        ToggleAction::Reshow => show_panel(app, t0),
        ToggleAction::RequestHide => request_hide(app),
        ToggleAction::HotkeyPressed => crate::events::emit(app, crate::events::HOTKEY_PRESSED, ()),
        ToggleAction::Ignore => {}
    }
}

fn request_hide(app: &AppHandle) {
    let req = REQ_SEQ.fetch_add(1, Ordering::Relaxed);
    HIDE_REQ.store(req, Ordering::Relaxed);
    crate::events::emit(app, crate::events::PANEL_HIDE_REQUESTED, ());
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(HIDE_FALLBACK);
        // 期间隐藏过或重新唤起过（编号已变）就不再动
        if HIDE_REQ.load(Ordering::Relaxed) == req {
            log::warn!("前端未响应隐藏请求，后端直接隐藏");
            hide(&app);
        }
    });
}

pub fn theme_of(a: Appearance) -> Option<Theme> {
    match a {
        Appearance::System => None,
        Appearance::Light => Some(Theme::Light),
        Appearance::Dark => Some(Theme::Dark),
    }
}

/// 窗口外观跟随设置（毛玻璃明暗随之变化）；失败只记日志。
pub fn apply_theme(app: &AppHandle, a: Appearance) {
    on_main(app, move |app| {
        if let Some(w) = window(app)
            && let Err(e) = w.set_theme(theme_of(a))
        {
            log::warn!("设置面板外观失败：{e}");
        }
    });
}

/// 物理尺寸换成逻辑像素；不可见或小于 720 × 520 时不记。
pub fn size_to_remember(phys: (u32, u32), sf: f64, visible: bool) -> Option<(f64, f64)> {
    let size = (phys.0 as f64 / sf, phys.1 as f64 / sf);
    (visible && size.0 >= 720.0 && size.1 >= 520.0).then_some(size)
}

/// 最近一次尺寸变化的编号；防抖期间又变了就放弃这次。
static RESIZE_SEQ: AtomicU64 = AtomicU64::new(0);
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(300);

/// 用户拖动改变面板尺寸后记住它；300 ms 防抖，失败只记日志。
pub fn remember_size(app: &AppHandle) {
    let seq = RESIZE_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(RESIZE_DEBOUNCE);
        if RESIZE_SEQ.load(Ordering::Relaxed) != seq {
            return;
        }
        let Some(w) = window(&app) else { return };
        let read = || -> tauri::Result<_> {
            let p = w.inner_size()?;
            Ok(size_to_remember(
                (p.width, p.height),
                w.scale_factor()?,
                w.is_visible()?,
            ))
        };
        let size = match read() {
            Ok(Some(size)) => size,
            Ok(None) => return,
            Err(e) => return log::warn!("读取面板尺寸失败：{e}"),
        };
        let state = app.state::<AppState>();
        // 持 settings 锁写库，并发的 save_settings 不会被这里的旧字段覆盖（锁序 settings → store，无反向路径）
        let mut s = state.settings();
        // 显示时 place() 按设置改尺寸也会触发 Resized，尺寸没变就不写
        if same_size(s.panel_size, size) {
            return;
        }
        s.panel_size = size;
        if let Err(e) = state.store().save_settings(&s) {
            log::warn!("保存面板尺寸失败：{e}");
        }
    });
}

/// 逻辑尺寸相差不超过 1 px 视为相同（分数缩放下物理像素换算有舍入）。
fn same_size(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() <= 1.0 && (a.1 - b.1).abs() <= 1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    const SIZE: (f64, f64) = (960.0, 640.0);

    #[test]
    fn size_to_remember_ignores_hidden_and_too_small() {
        assert_eq!(
            size_to_remember((1920, 1280), 2.0, true),
            Some((960.0, 640.0))
        );
        assert_eq!(size_to_remember((1920, 1280), 2.0, false), None);
        assert_eq!(size_to_remember((1400, 1000), 2.0, true), None); // 700 × 500 < 720 × 520
        assert!(same_size(SIZE, (960.5, 639.5)));
        assert!(!same_size(SIZE, (962.0, 640.0)));
    }

    #[test]
    fn toggle_action_by_trigger() {
        use Trigger::*;
        let show = |then_hotkey| ToggleAction::Show { then_hotkey };
        assert_eq!(
            toggle_action(true, false, true, Hotkey, 9999),
            ToggleAction::HotkeyPressed
        );
        assert_eq!(
            toggle_action(true, false, true, Tray, 9999),
            ToggleAction::RequestHide
        ); // 托盘不算试按
        assert_eq!(
            toggle_action(true, false, true, Relaunch, 9999),
            ToggleAction::RequestHide
        );
        assert_eq!(
            toggle_action(true, true, false, Hotkey, 9999),
            ToggleAction::Reshow
        );
        assert_eq!(
            toggle_action(false, false, false, Tray, 120),
            ToggleAction::Ignore
        );
        assert_eq!(toggle_action(false, false, false, Tray, 300), show(false));
        assert_eq!(toggle_action(false, false, false, Hotkey, 120), show(false));
        assert_eq!(toggle_action(false, false, true, Hotkey, 9999), show(true));
        assert_eq!(toggle_action(false, false, true, Tray, 9999), show(false));
    }
    #[test]
    fn windows_material_falls_back_but_win10_stays_opaque() {
        use window_vibrancy::Error as E;
        let never = || -> Result<(), E> { panic!("Windows 10 不试 Acrylic") };
        assert_eq!(windows_material(Ok(()), never), "glass");
        assert_eq!(
            windows_material(Err(E::UnsupportedPlatformVersion("win10")), never),
            "opaque"
        );
        assert_eq!(
            windows_material(Err(E::NotMainThread("x")), || Ok(())),
            "glass"
        );
        assert_eq!(
            windows_material(Err(E::NotMainThread("x")), || Err(E::NotMainThread("y"))),
            "opaque"
        );
    }

    #[test]
    fn theme_of_maps_appearance() {
        assert_eq!(theme_of(Appearance::System), None);
        assert_eq!(theme_of(Appearance::Light), Some(tauri::Theme::Light));
        assert_eq!(theme_of(Appearance::Dark), Some(tauri::Theme::Dark));
    }

    #[test]
    fn centered_horizontally_top_at_one_sixth() {
        let work = Rect {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        assert_eq!(place_in_work_area(work, SIZE), (480.0, 180.0));
    }

    #[test]
    fn respects_monitor_origin() {
        let work = Rect {
            x: 1920.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        assert_eq!(place_in_work_area(work, SIZE), (2400.0, 180.0));
    }

    #[test]
    fn clamps_into_small_work_area() {
        let work = Rect {
            x: 0.0,
            y: 25.0,
            w: 1280.0,
            h: 695.0,
        }; // 1280 × 720 减去 25 px 菜单栏
        let (x, y) = place_in_work_area(work, SIZE);
        assert_eq!(x, 160.0);
        assert_eq!(y, 80.0); // 25 + 695 − 640，贴底而不是出屏
    }

    #[test]
    fn mixed_dpi_macos_hits_external_monitor() {
        let cursor = (5000.0, 100.0);
        // 1x 外接屏在 Retina 主屏（1512 pt 宽）右侧
        assert!(monitor_contains(
            cursor,
            2.0,
            (1512.0, 0.0),
            (1920.0, 1080.0),
            1.0,
            true
        ));
        assert!(!monitor_contains(
            cursor,
            2.0,
            (0.0, 0.0),
            (3024.0, 1964.0),
            2.0,
            true
        ));
    }

    #[test]
    fn physical_space_hit_test_windows() {
        let (pos, size) = ((1920.0, 0.0), (1920.0, 1080.0));
        assert!(monitor_contains(
            (2500.0, 100.0),
            1.5,
            pos,
            size,
            1.0,
            false
        ));
        assert!(!monitor_contains(
            (1000.0, 100.0),
            1.5,
            pos,
            size,
            1.0,
            false
        ));
    }
}
