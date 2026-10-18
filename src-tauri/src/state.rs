use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use xpaste_core::Settings;

/// 由 Tauri 托管、各命令共享的应用状态。
pub struct AppState {
    pub settings: Mutex<Settings>,
    pub paused: AtomicBool,
}

impl AppState {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings: Mutex::new(settings),
            paused: AtomicBool::new(false),
        }
    }
}
