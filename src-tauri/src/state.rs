use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64};

use xpaste_core::Settings;

/// 由 Tauri 托管、各命令共享的应用状态。
pub struct AppState {
    pub settings: Mutex<Settings>,
    pub paused: AtomicBool,
    /// 本应用最近一次写剪切板后的序号，监听时据此跳过自身写入
    pub last_write_seq: AtomicU64,
}

impl AppState {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings: Mutex::new(settings),
            paused: AtomicBool::new(false),
            last_write_seq: AtomicU64::new(0),
        }
    }
}
