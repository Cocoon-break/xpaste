mod assets;
mod clipboard;
mod clock;
mod commands;
mod enrich;
pub mod error;
mod events;
mod hotkey;
mod housekeeping;
mod ocr;
mod panel;
mod rec;
mod shot;
pub mod state;
mod tray;

use state::AppState;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, WindowEvent};
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
    let builder = builder
        .plugin(tauri_nspanel::init())
        // 遮罩页面的内容进程崩溃：会话不会再结束，结束它并重新加载页面（M23 规则 1）
        .on_web_content_process_terminate(shot::overlay_crashed);
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
            // 贴图只在本次运行内有效（M23 规则 9）
            if let Err(e) = shot::clear_pins(&data_dir.join("pins")) {
                log::warn!("清空贴图目录失败：{e}");
            }
            if isolated {
                log::info!("数据目录已由 XPASTE_DATA_DIR 覆盖，不启用单实例");
                // tauri.conf.json 的 assetProtocol.scope 只覆盖默认数据目录
                for sub in ["images", "thumbs", "icons", "pins"] {
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
            let (hotkey, shot_hotkey, rec_hotkey, appearance) = (
                settings.hotkey.clone(),
                settings.shot_hotkey.clone(),
                settings.rec_hotkey.clone(),
                settings.appearance,
            );
            let enrich = enrich::start(app.handle().clone());
            let ocr = ocr::start(app.handle().clone());
            app.manage(AppState::new(
                settings,
                store,
                enrich,
                ocr.clone(),
                data_dir,
            ));
            ocr.schedule_backfill(app.handle(), ocr::BACKFILL_DELAY);
            housekeeping::start(app.handle().clone());
            #[cfg(target_os = "macos")]
            panel::setup_macos(app)?;
            #[cfg(windows)]
            panel::setup_windows(app);
            panel::apply_theme(app.handle(), appearance);
            if let Err(e) = tray::setup(app) {
                log::warn!("托盘图标创建失败：{e}");
            }
            if let Err(e) = hotkey::register(app.handle(), &hotkey, &shot_hotkey, &rec_hotkey) {
                log::warn!("快捷键注册失败：{e}");
                // register 只报第一个失败，截图、录屏是否注册上另查；它们失败只提示，不标记托盘
                let panel_failed = matches!(e, error::AppError::HotkeyTaken);
                let (shot_failed, rec_failed) =
                    hotkey::unregistered(app.handle(), &shot_hotkey, &rec_hotkey);
                if let Some(body) = hotkey::failure_notice(panel_failed, shot_failed, rec_failed) {
                    notify(app.handle(), &body);
                }
                if panel_failed {
                    tray::mark_hotkey_failed();
                }
            }
            clipboard::start(app.handle().clone());
            shot::prewarm(app.handle());
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
        // 设置里选截图文件夹
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    let (p, s, r) = {
                        let st = app.state::<AppState>();
                        let s = st.settings();
                        (
                            s.hotkey.clone(),
                            s.shot_hotkey.clone(),
                            s.rec_hotkey.clone(),
                        )
                    };
                    match hotkey::role_of(shortcut, &p, &s, &r) {
                        Some(hotkey::Role::Panel) => {
                            panel::toggle(app, timing.then(now_unix_ms), panel::Trigger::Hotkey)
                        }
                        Some(hotkey::Role::Shot) => hotkey::shot_pressed(app),
                        Some(hotkey::Role::Rec) => hotkey::rec_pressed(app),
                        None => {}
                    }
                })
                .build(),
        )
        .on_window_event(|window, event| {
            // 遮罩不能被关掉（Windows 上 Alt+F4）：关掉后会话不会再结束、截图快捷键失灵，当作退出截图（M23 规则 8）
            if shot::overlay_index(window.label()).is_some()
                && let WindowEvent::CloseRequested { api, .. } = event
            {
                api.prevent_close();
                shot::end(window.app_handle());
            }
            // 截图遮罩（shot-*）与贴图（pin-*）失焦不处理
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
            commands::settings::installed_apps,
            commands::panel::hide_panel,
            commands::panel::frontend_ready,
            commands::panel::set_dialog_open,
            commands::panel::accessibility_granted,
            commands::panel::request_accessibility,
            commands::panel::screen_capture_granted,
            commands::panel::request_screen_capture,
            commands::panel::open_url,
            commands::panel::reveal_file,
            commands::records::get_record,
            commands::records::search,
            commands::records::set_pinned,
            commands::records::delete,
            commands::records::undo_delete,
            commands::records::save_snippet,
            commands::records::create_snippet,
            commands::records::update_snippet,
            commands::records::clear_history,
            commands::records::count_unpinned,
            commands::records::copy,
            commands::records::copy_text_plain,
            commands::records::reocr,
            commands::records::ocr_progress,
            commands::shot::shot_frame,
            commands::shot::shot_begin_payload,
            commands::shot::shot_ready,
            commands::shot::shot_alive,
            commands::shot::shot_cancel,
            commands::shot::overlay_begin,
            commands::shot::shot_finish,
            commands::shot::pin_close,
            commands::shot::pin_copy,
            commands::shot::pin_save,
            commands::shot::pin_set_bounds,
            commands::shot::pin_button_down,
            commands::shot::rec_start,
            commands::shot::rec_frame
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| match event {
            // macOS 重开已运行的 .app（Finder / Spotlight）不起第二个进程，而是发 Reopen
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => panel::toggle(app, None, panel::Trigger::Relaunch),
            // 录制、开始或停止进行中时拦下这次退出（托盘“退出”也走这里）：rec 先停止、把文件写完再退出，
            // 不然 mp4 打不开（M24 规则 8）。守卫里的 hold_exit 每次都会执行
            tauri::RunEvent::ExitRequested { api, .. } if rec::hold_exit(app) => api.prevent_exit(),
            _ => {}
        });
}

/// 系统通知，标题“xPaste”；发送失败只记日志。
pub(crate) fn notify(app: &AppHandle, body: &str) {
    if let Err(e) = app
        .notification()
        .builder()
        .title("xPaste")
        .body(body)
        .show()
    {
        log::warn!("发送系统通知失败：{e}");
    }
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
