use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64};

use xpaste_core::Settings;

/// 由 Tauri 托管、各命令共享的应用状态。
pub struct AppState {
    pub settings: Mutex<Settings>,
    pub paused: AtomicBool,
    /// 本应用最近一次写剪切板后的序号，监听时据此跳过自身写入；`u64::MAX` 表示从未写过
    pub last_write_seq: AtomicU64,
    /// 本应用正在写剪切板（序号尚未存入 `last_write_seq`），监听期间到来的变化一律视为自身写入
    pub writing: AtomicBool,
    /// 唤起面板前的前台窗口（Windows HWND），粘贴时切回；0 表示无（macOS 恒为 0）
    pub previous_window: AtomicIsize,
}

impl AppState {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings: Mutex::new(settings),
            paused: AtomicBool::new(false),
            last_write_seq: AtomicU64::new(u64::MAX),
            writing: AtomicBool::new(false),
            previous_window: AtomicIsize::new(0),
        }
    }
}
