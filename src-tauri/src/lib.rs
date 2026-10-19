mod assets;
mod clipboard;
mod clock;
mod commands;
mod enrich;
pub mod error;
mod events;
mod hotkey;
mod housekeeping;
mod panel;
pub mod state;
mod tray;

use state::AppState;
use tauri::{Manager, WindowEvent};
use tauri_plugin_global_shortcut::ShortcutState;
use tauri_plugin_log::{Target, TargetKind};
use xpaste_core::Store;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());
    builder
        .setup(|app| {
            let data_dir = app.path().data_dir()?.join("xPaste");
            let logs = data_dir.join("logs");
            // Builder::new() 自带 Stdout + LogDir，先清掉，只留下面显式加的目标
            let builder = tauri_plugin_log::Builder::new()
                .clear_targets()
                .level(log::LevelFilter::Info)
                .max_file_size(1_000_000)
                .target(Target::new(TargetKind::Folder {
                    path: logs,
                    file_name: None,
                }));
            // 开发构建额外输出到终端和 WebView 控制台
            let builder = if cfg!(debug_assertions) {
                builder
                    .target(Target::new(TargetKind::Stdout))
                    .target(Target::new(TargetKind::Webview))
            } else {
                builder
            };
            app.handle().plugin(builder.build())?;
            log::info!("xPaste 启动，平台 {}", xpaste_platform::OS);
            let store = Store::open(&data_dir)?;
            let settings = store.load_settings();
            let hotkey = settings.hotkey.clone();
            let enrich = enrich::start(app.handle().clone());
            app.manage(AppState::new(settings, store, enrich, data_dir));
            housekeeping::start(app.handle().clone());
            #[cfg(target_os = "macos")]
            panel::setup_macos(app)?;
            if let Err(e) = tray::setup(app) {
                log::warn!("托盘图标创建失败：{e}");
            }
            if let Err(e) = hotkey::register(app.handle(), &hotkey) {
                // 系统通知在阶段 5
                log::warn!("快捷键注册失败：{e}");
            }
            clipboard::start(app.handle().clone());
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
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
        .invoke_handler(tauri::generate_handler![
            commands::settings::bootstrap,
            commands::settings::save_settings,
            commands::settings::prune_preview,
            commands::settings::set_paused,
            commands::settings::finish_onboarding,
            commands::panel::hide_panel,
            commands::panel::open_accessibility_settings,
            commands::panel::open_url,
            commands::panel::reveal_file,
            commands::records::get_record,
            commands::records::search,
            commands::records::set_pinned,
            commands::records::delete,
            commands::records::undo_delete,
            commands::records::clear_history,
            commands::records::count_unpinned,
            commands::records::paste_text
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
