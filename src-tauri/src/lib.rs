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
    let override_ = data_dir_override(std::env::var_os("XPASTE_DATA_DIR"));
    let timing = timing_enabled(std::env::var_os("XPASTE_TIMING"));
    let builder = tauri::Builder::default();
    // 单实例插件必须第一个注册：再次启动时切换已运行实例的面板。
    // 它按 bundle id 加锁，隔离数据目录的开发版不注册，否则会和正在运行的发布版互相切换
    let builder = if override_.is_some() {
        builder
    } else {
        builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            panel::toggle(app, None, panel::Trigger::Relaunch)
        }))
    };
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());
    builder
        .setup(move |app| {
            let isolated = override_.is_some();
            let data_dir = match override_ {
                Some(d) => d,
                None => app.path().data_dir()?.join("xPaste"),
            };
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
            if timing {
                log::info!("唤起计时已由 XPASTE_TIMING 开启");
            }
            if isolated {
                log::info!("数据目录已由 XPASTE_DATA_DIR 覆盖，不启用单实例");
                // tauri.conf.json 的 assetProtocol.scope 只覆盖默认数据目录
                for sub in ["images", "thumbs", "icons"] {
                    if let Err(e) = app
                        .asset_protocol_scope()
                        .allow_directory(data_dir.join(sub), true)
                    {
                        log::warn!("放行隔离数据目录 {sub} 失败");
                        log::debug!("asset 作用域错误：{e}");
                    }
                }
            }
            let store = Store::open(&data_dir)?;
            let first_run = matches!(store.kv_get("onboarded"), Ok(None));
            let settings = store.load_settings();
            let (hotkey, appearance) = (settings.hotkey.clone(), settings.appearance);
            let enrich = enrich::start(app.handle().clone());
            app.manage(AppState::new(settings, store, enrich, data_dir));
            housekeeping::start(app.handle().clone());
            #[cfg(target_os = "macos")]
            panel::setup_macos(app)?;
            #[cfg(windows)]
            panel::setup_windows(app);
            panel::apply_theme(app.handle(), appearance);
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
                tray::mark_hotkey_failed();
            }
            clipboard::start(app.handle().clone());
            // 首次启动直接显示面板与引导（AC-M18-01）
            if first_run {
                app.state::<AppState>()
                    .onboarding
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                panel::show(app.handle(), None, panel::Trigger::Relaunch);
            }
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        panel::toggle(app, timing.then(now_unix_ms), panel::Trigger::Hotkey);
                    }
                })
                .build(),
        )
        .on_window_event(|window, event| {
            if window.label() != "panel" {
                return;
            }
            match event {
                // 对话框打开时（例如设置里切去别的应用查东西）失焦不隐藏
                WindowEvent::Focused(false) => {
                    let app = window.app_handle();
                    if !app
                        .state::<AppState>()
                        .dialog_open
                        .load(std::sync::atomic::Ordering::SeqCst)
                    {
                        panel::hide(app);
                    }
                }
                WindowEvent::Resized(_) => panel::remember_size(window.app_handle()),
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::settings::bootstrap,
            commands::settings::save_settings,
            commands::settings::prune_preview,
            commands::settings::set_paused,
            commands::settings::finish_onboarding,
            commands::settings::preview_appearance,
            commands::settings::installed_apps,
            commands::panel::hide_panel,
            commands::panel::set_dialog_open,
            commands::panel::open_accessibility_settings,
            commands::panel::accessibility_granted,
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
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| {
            // macOS 重开已运行的 .app（Finder / Spotlight）不起第二个进程，而是发 Reopen
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                panel::toggle(app, None, panel::Trigger::Relaunch);
            }
            let _ = (app, &event);
        });
}

/// 环境变量非空（去首尾空白后）时用它作数据目录，否则用默认目录；开发版隔离真实数据用。
pub fn data_dir_override(v: Option<std::ffi::OsString>) -> Option<PathBuf> {
    v.filter(|s| !s.to_string_lossy().trim().is_empty())
        .map(PathBuf::from)
}

/// 同 `data_dir_override` 的口径：XPASTE_TIMING 去首尾空白后非空才开启唤起与搜索计时。
pub fn timing_enabled(v: Option<std::ffi::OsString>) -> bool {
    v.is_some_and(|s| !s.to_string_lossy().trim().is_empty())
}

/// 收到快捷键时的 Unix 毫秒，前端据此算唤起耗时。
fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_override_wins_only_when_non_empty() {
        assert_eq!(
            data_dir_override(Some("/tmp/xp".into())),
            Some(PathBuf::from("/tmp/xp"))
        );
        assert_eq!(data_dir_override(Some("".into())), None);
        assert_eq!(data_dir_override(Some("  ".into())), None);
        assert_eq!(data_dir_override(None), None);
    }

    #[test]
    fn timing_enabled_only_when_non_empty() {
        assert!(timing_enabled(Some("1".into())));
        assert!(!timing_enabled(Some("".into())));
        assert!(!timing_enabled(Some("  ".into())));
        assert!(!timing_enabled(None));
    }
}
