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
    open: MenuItem<Wry>,
    pause: MenuItem<Wry>,
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

/// 快捷键改成功后更新第一项文字，并恢复“打开 xPaste”的点击行为。
pub fn set_hotkey_label(shortcut: &str) {
    HOTKEY_FAILED.store(false, Ordering::SeqCst);
    set_text(|h| &h.open, &open_label(&hotkey::label(shortcut)));
}

pub fn mark_hotkey_failed() {
    HOTKEY_FAILED.store(true, Ordering::SeqCst);
    set_text(|h| &h.open, "更换唤起快捷键…");
}

fn open_settings(app: &AppHandle) {
    panel::show(app, None);
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
    let quit = MenuItem::with_id(app, "quit", "退出 xPaste", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &pause,
            &clear,
            &settings,
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
            "open" => panel::show(app, None),
            "pause" => {
                let paused = app.state::<AppState>().paused.load(Ordering::SeqCst);
                crate::commands::settings::set_paused_inner(app, !paused);
            }
            "clear" => {
                panel::show(app, None);
                emit(app, CONFIRM_CLEAR, ());
            }
            "settings" => open_settings(app),
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
                panel::toggle(tray.app_handle(), None);
            }
        })
        .build(app)?;
    let _ = TRAY.set(Handles { open, pause });
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
    }
}
