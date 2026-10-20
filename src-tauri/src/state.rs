use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64};
use std::sync::{Mutex, MutexGuard, PoisonError, mpsc};

use xpaste_core::{Settings, Store};

use crate::enrich;

/// 由 Tauri 托管、各命令共享的应用状态。
/// 锁顺序：不同时持有 `settings` 与 `store`；持锁期间不发事件。
pub struct AppState {
    pub settings: Mutex<Settings>,
    pub store: Mutex<Store>,
    pub enrich: mpsc::Sender<enrich::Job>,
    /// `<应用数据目录>/xPaste`，与 `store.dir` 相同
    pub data_dir: PathBuf,
    pub paused: AtomicBool,
    /// 本应用最近一次写剪切板后的序号，监听时据此跳过自身写入；`u64::MAX` 表示从未写过
    pub last_write_seq: AtomicU64,
    /// 本应用正在写剪切板（序号尚未存入 `last_write_seq`），监听期间到来的变化一律视为自身写入
    pub writing: AtomicBool,
    /// 唤起面板前的前台窗口（Windows HWND），粘贴时切回；0 表示无（macOS 恒为 0）
    pub previous_window: AtomicIsize,
    /// 首次启动引导进行中：快捷键不隐藏面板，改发 `hotkey-pressed`
    pub onboarding: AtomicBool,
    /// 面板上有对话框打开：失焦与快捷键都不隐藏面板（快捷键改发 `hotkey-pressed`）；面板隐藏时复位
    pub dialog_open: AtomicBool,
}

impl AppState {
    pub fn new(
        settings: Settings,
        store: Store,
        enrich: mpsc::Sender<enrich::Job>,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            settings: Mutex::new(settings),
            store: Mutex::new(store),
            enrich,
            data_dir,
            paused: AtomicBool::new(false),
            last_write_seq: AtomicU64::new(u64::MAX),
            writing: AtomicBool::new(false),
            previous_window: AtomicIsize::new(0),
            onboarding: AtomicBool::new(false),
            dialog_open: AtomicBool::new(false),
        }
    }

    pub fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn settings(&self) -> MutexGuard<'_, Settings> {
        self.settings.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 测试用：内存数据库、无人接收的补全通道。
    #[cfg(test)]
    pub fn for_test() -> Self {
        let dir = std::env::temp_dir().join(format!("xpaste-app-test-{}", std::process::id()));
        let store = Store::open_in_memory(&dir).expect("内存数据库");
        Self::new(Settings::default(), store, mpsc::channel().0, dir)
    }
}
