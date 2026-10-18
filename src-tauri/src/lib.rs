mod commands;
pub mod error;
pub mod state;

use state::AppState;
use tauri::Manager;
use tauri_plugin_log::{Target, TargetKind};
use xpaste_core::Settings;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
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
            Ok(())
        })
        .manage(AppState::new(Settings::default()))
        .invoke_handler(tauri::generate_handler![commands::settings::bootstrap])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
