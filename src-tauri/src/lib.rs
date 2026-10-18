mod commands;
pub mod error;
pub mod state;

use state::AppState;
use xpaste_core::Settings;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::new(Settings::default()))
        .invoke_handler(tauri::generate_handler![commands::settings::bootstrap])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
