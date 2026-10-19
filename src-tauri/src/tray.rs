//! 菜单栏 / 托盘图标（规格说明 M17）。图标由 `make icons` 从 docs/brand 生成。
//! 暂停记录、清空历史、设置三项菜单和暂停样式图标随阶段 5 的对应功能接入。

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, include_image};

use crate::panel;

pub fn setup(app: &App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "打开 xPaste", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出 xPaste", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    // macOS 用模板图，系统按菜单栏明暗自动反色；Windows 用彩色图
    #[cfg(target_os = "macos")]
    let icon = include_image!("icons/tray/macos.png");
    #[cfg(not(target_os = "macos"))]
    let icon = include_image!("icons/tray/windows.png");

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("xPaste")
        .menu(&menu)
        // macOS 点击打开菜单；Windows 左键切换面板、右键打开菜单
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => panel::show(app, None),
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
    Ok(())
}
