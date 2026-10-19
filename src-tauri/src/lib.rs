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
use std::path::PathBuf;
use tauri::{Manager, WindowEvent};
use tauri_plugin_global_shortcut::ShortcutState;
use tauri_plugin_log::{Target, TargetKind};
use tauri_plugin_notification::NotificationExt;
use xpaste_core::Store;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 单实例插件必须第一个注册：再次启动时切换已运行实例的面板
    let builder =
        tauri::Builder::default().plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            panel::toggle(app, None)
        }));
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());
    builder
        .setup(|app| {
            let override_ = std::env::var_os("XPASTE_DATA_DIR");
            let default_dir = app.path().data_dir()?.join("xPaste");
            let data_dir = data_dir_from(override_, default_dir.clone());
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
            if data_dir != default_dir {
                log::info!("数据目录已由 XPASTE_DATA_DIR 覆盖");
            }
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
                log::warn!("快捷键注册失败：{e}");
                if let Err(e) = app
                    .notification()
                    .builder()
                    .title("xPaste")
                    .body("唤起快捷键被其他应用占用，请在设置中更换")
                    .show()
                {
                    log::warn!("发送快捷键注册失败通知失败：{e}");
                }
                tray::mark_hotkey_failed(app.handle());
            }
            clipboard::start(app.handle().clone());
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
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
            commands::panel::request_accessibility,
            commands::panel::open_url,
            commands::panel::reveal_file,
            commands::records::get_record,
            commands::records::search,
            commands::records::set_pinned,
            commands::records::delete,
            commands::records::undo_delete,
            commands::records::clear_history,
            commands::records::count_unpinned,
            commands::records::copy,
            commands::records::copy_text
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// 环境变量非空（去首尾空白后）时用它作数据目录，否则用默认目录；开发版隔离真实数据用。
pub fn data_dir_from(override_: Option<std::ffi::OsString>, default: PathBuf) -> PathBuf {
    match override_ {
        Some(s) if !s.to_string_lossy().trim().is_empty() => PathBuf::from(s),
        _ => default,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_override_wins_only_when_non_empty() {
        let d = PathBuf::from("/default/xPaste");
        assert_eq!(
            data_dir_from(Some("/tmp/xp".into()), d.clone()),
            PathBuf::from("/tmp/xp")
        );
        assert_eq!(data_dir_from(Some("".into()), d.clone()), d);
        assert_eq!(data_dir_from(Some("  ".into()), d.clone()), d);
        assert_eq!(data_dir_from(None, d.clone()), d);
    }
}
