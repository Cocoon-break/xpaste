mod commands;
pub mod error;
mod hotkey;
mod panel;
pub mod state;

use state::AppState;
use tauri::{Manager, WindowEvent};
use tauri_plugin_global_shortcut::ShortcutState;
use tauri_plugin_log::{Target, TargetKind};
use xpaste_core::Settings;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());
    builder
        .setup(|app| {
            let logs = app.path().data_dir()?.join("xPaste").join("logs");
            // Builder::new() 自带 Stdout + LogDir，先清掉，只留下面显式加的目标
            let mut builder = tauri_plugin_log::Builder::new()
                .clear_targets()
                .level(log::LevelFilter::Info)
                .max_file_size(1_000_000)
                .target(Target::new(TargetKind::Folder {
                    path: logs,
                    file_name: None,
                }));
            #[cfg(debug_assertions)]
            {
                builder = builder
                    .target(Target::new(TargetKind::Stdout))
                    .target(Target::new(TargetKind::Webview));
            }
            app.handle().plugin(builder.build())?;
            log::info!("xPaste 启动，平台 {}", xpaste_platform::OS);
            #[cfg(target_os = "macos")]
            panel::setup_macos(app)?;
            let hotkey = app
                .state::<AppState>()
                .settings
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .hotkey
                .clone();
            if let Err(e) = hotkey::register(app.handle(), &hotkey) {
                // 系统通知在阶段 5
                log::warn!("快捷键注册失败：{e}");
            }
            Ok(())
        })
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        panel::toggle(app, t0());
                    }
                })
                .build(),
        )
        .on_window_event(|window, event| {
            if window.label() == "panel"
                && let WindowEvent::Focused(false) = event
            {
                panel::hide(window.app_handle());
            }
        })
        .manage(AppState::new(Settings::default()))
        .invoke_handler(tauri::generate_handler![
            commands::settings::bootstrap,
            commands::panel::hide_panel
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// 收到快捷键时的 Unix 毫秒，仅开发构建用于测唤起耗时。
fn t0() -> Option<u64> {
    #[cfg(debug_assertions)]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_millis() as u64)
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}
