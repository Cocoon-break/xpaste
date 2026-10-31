//! 菜单栏 / 托盘图标（规格说明 M17）。图标由 `make icons` 从 docs/brand 生成。

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, Wry, include_image};

use crate::events::{CONFIRM_CLEAR, OPEN_SETTINGS, emit};
use crate::state::AppState;
use crate::{hotkey, panel};

struct Handles {
    menu: Menu<Wry>,
    open: MenuItem<Wry>,
    pause: MenuItem<Wry>,
    /// “停止录制”：只在录制中插在第一项（M24）
    rec_stop: MenuItem<Wry>,
}

static TRAY: OnceLock<Handles> = OnceLock::new();
/// 启动时快捷键注册失败：第一项改为“更换唤起快捷键…”，点击打开设置。
static HOTKEY_FAILED: AtomicBool = AtomicBool::new(false);

pub fn open_label(label: &str) -> String {
    format!("打开 xPaste（{label}）")
}

pub fn pause_label(paused: bool) -> &'static str {
    if paused {
        "继续记录"
    } else {
        "暂停记录"
    }
}

/// 菜单“停止录制（<录屏快捷键>）”；没设录屏快捷键时不带括号。
pub fn rec_stop_label(hotkey_label: &str) -> String {
    if hotkey_label.is_empty() {
        "停止录制".into()
    } else {
        format!("停止录制（{hotkey_label}）")
    }
}

pub fn version_label(v: &str) -> String {
    format!("xPaste {v}")
}

// macOS 用模板图，系统按菜单栏明暗自动反色；Windows 用彩色图
fn icon(paused: bool) -> Image<'static> {
    #[cfg(target_os = "macos")]
    let (normal, dim) = (
        include_image!("icons/tray/macos.png"),
        include_image!("icons/tray/macos-paused.png"),
    );
    #[cfg(not(target_os = "macos"))]
    let (normal, dim) = (
        include_image!("icons/tray/windows.png"),
        include_image!("icons/tray/windows-paused.png"),
    );
    if paused { dim } else { normal }
}

fn set_text(item: impl FnOnce(&Handles) -> &MenuItem<Wry>, text: &str) {
    if let Some(h) = TRAY.get()
        && let Err(e) = item(h).set_text(text)
    {
        log::warn!("更新托盘菜单文字失败：{e}");
    }
}

/// 暂停状态变化：换菜单文字和图标。
pub fn set_paused(app: &AppHandle, on: bool) {
    set_text(|h| &h.pause, pause_label(on));
    if let Some(tray) = app.tray_by_id("main") {
        let r = tray
            .set_icon(Some(icon(on)))
            .and_then(|()| tray.set_icon_as_template(cfg!(target_os = "macos")));
        if let Err(e) = r {
            log::warn!("更新托盘图标失败：{e}");
        }
    }
}

/// 录制中的菜单栏标题（`rec::tray_title`）；`None` 清空。
pub fn set_recording(app: &AppHandle, elapsed: Option<std::time::Duration>) {
    if let Some(tray) = app.tray_by_id("main")
        && let Err(e) = tray.set_title(elapsed.map(crate::rec::tray_title))
    {
        log::warn!("更新托盘标题失败：{e}");
    }
}

/// 录制中在菜单第一项插入“停止录制”，停止后移除。重复调用不重复插入。
pub fn show_stop_item(app: &AppHandle, on: bool) {
    let Some(h) = TRAY.get() else {
        return;
    };
    let shown = h.menu.get(h.rec_stop.id()).is_some();
    let r = if on && !shown {
        let key = hotkey::label(&app.state::<AppState>().settings().rec_hotkey);
        h.rec_stop
            .set_text(rec_stop_label(&key))
            .and_then(|()| h.menu.insert(&h.rec_stop, 0))
    } else if !on && shown {
        h.menu.remove(&h.rec_stop)
    } else {
        Ok(())
    };
    if let Err(e) = r {
        log::warn!("更新托盘“停止录制”失败：{e}");
    }
}

/// 快捷键改成功后更新第一项文字，并恢复“打开 xPaste”的点击行为。
pub fn set_hotkey_label(shortcut: &str) {
    HOTKEY_FAILED.store(false, Ordering::SeqCst);
    set_text(|h| &h.open, &open_label(&hotkey::label(shortcut)));
}

pub fn hotkey_failed() -> bool {
    HOTKEY_FAILED.load(Ordering::SeqCst)
}

pub fn mark_hotkey_failed() {
    HOTKEY_FAILED.store(true, Ordering::SeqCst);
    set_text(|h| &h.open, "更换唤起快捷键…");
}

fn open_settings(app: &AppHandle) {
    panel::show(app, None, panel::Trigger::Tray);
    emit(app, OPEN_SETTINGS, ());
}

pub fn setup(app: &App) -> tauri::Result<()> {
    let (open_text, paused) = {
        let state = app.state::<AppState>();
        (
            open_label(&hotkey::label(&state.settings().hotkey)),
            state.paused.load(Ordering::SeqCst),
        )
    };
    let open = MenuItem::with_id(app, "open", open_text, true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", pause_label(paused), true, None::<&str>)?;
    let clear = MenuItem::with_id(app, "clear", "清空历史…", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "设置…", true, None::<&str>)?;
    let translate = MenuItem::with_id(app, "translate", "翻译…", true, None::<&str>)?;
    let version = MenuItem::with_id(
        app,
        "version",
        version_label(&app.package_info().version.to_string()),
        false,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "退出 xPaste", true, None::<&str>)?;
    let rec_stop = MenuItem::with_id(app, "rec-stop", rec_stop_label(""), true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &translate,
            &PredefinedMenuItem::separator(app)?,
            &pause,
            &clear,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &version,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    TrayIconBuilder::with_id("main")
        .icon(icon(paused))
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("xPaste")
        .menu(&menu)
        // macOS 点击打开菜单；Windows 左键切换面板、右键打开菜单
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" if HOTKEY_FAILED.load(Ordering::SeqCst) => open_settings(app),
            "open" => panel::show(app, None, panel::Trigger::Tray),
            "pause" => {
                let paused = app.state::<AppState>().paused.load(Ordering::SeqCst);
                crate::commands::settings::set_paused_inner(app, !paused);
            }
            "clear" => {
                panel::show(app, None, panel::Trigger::Tray);
                emit(app, CONFIRM_CLEAR, ());
            }
            "settings" => open_settings(app),
            // 输入卡片（M26 规则 20）
            "translate" => crate::trcard::open_input(app),
            "rec-stop" => crate::rec::stop(app),
            // 录制中退出由 ExitRequested 里的 rec::hold_exit 先停止、写完文件再退出（M24 规则 8）
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if cfg!(not(target_os = "macos"))
                && let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
            {
                panel::toggle(tray.app_handle(), None, panel::Trigger::Tray);
            }
        })
        .build(app)?;
    let _ = TRAY.set(Handles {
        menu,
        open,
        pause,
        rec_stop,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(open_label("⌘ + Shift + V"), "打开 xPaste（⌘ + Shift + V）");
        assert_eq!(pause_label(true), "继续记录");
        assert_eq!(pause_label(false), "暂停记录");
        assert_eq!(version_label("0.1.0"), "xPaste 0.1.0");
        assert_eq!(rec_stop_label("⌥ + Shift + R"), "停止录制（⌥ + Shift + R）");
        assert_eq!(rec_stop_label(""), "停止录制");
    }

    // 托盘未建时 set_text 不做事，只验失败标志；本文件外的测试不碰 HOTKEY_FAILED
    #[test]
    fn hotkey_failed_cleared_by_label() {
        mark_hotkey_failed();
        assert!(hotkey_failed());
        set_hotkey_label("CmdOrCtrl+Shift+V");
        assert!(!hotkey_failed());
    }
}
