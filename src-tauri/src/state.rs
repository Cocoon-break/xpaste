use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64};
use std::sync::{Mutex, MutexGuard, PoisonError, mpsc};

use xpaste_core::{Settings, Store};

use crate::{enrich, ocr, rec, shot, trcard};

/// 由 Tauri 托管、各命令共享的应用状态。
/// 锁顺序：不同时持有 `settings` 与 `store`；持锁期间不发事件。
pub struct AppState {
    pub settings: Mutex<Settings>,
    pub store: Mutex<Store>,
    pub enrich: mpsc::Sender<enrich::Job>,
    pub ocr: ocr::Handle,
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
    /// 面板上设置或确认框打开（引导不算）：失焦与快捷键都不隐藏面板（快捷键改发 `hotkey-pressed`）；面板隐藏时复位
    pub dialog_open: AtomicBool,
    /// 面板失焦也不隐藏：片段对话框开着（写了一半的内容不能丢，M25 规则 5），或正在用系统窗口选文件夹。
    /// 设置、确认框开着时失焦照常隐藏（M04 规则 3）；面板隐藏时复位
    pub blur_keeps: AtomicBool,
    /// 进行中的截图（冻结图与各屏遮罩就绪情况）
    pub shot: Mutex<shot::Session>,
    /// 进行中的录屏（M24）
    pub rec: Mutex<rec::RecState>,
    /// 正在翻译的记录 id → 这次请求的序号：同一条记录不发第二个请求（M26 规则 4）；
    /// 翻译中“换用…”换上新序号，旧请求据此中止、不存库（M26 规则 16）
    pub translating: Mutex<HashMap<i64, u64>>,
    /// 正在下载语言包：系统确认框可能抢走焦点，期间面板失焦不隐藏；同一时刻只下载一个（M26 规则 17）
    pub tr_downloading: AtomicBool,
    /// 划词翻译卡片的会话（M26 规则 19）
    pub trcard: Mutex<trcard::Session>,
}

impl AppState {
    pub fn new(
        settings: Settings,
        store: Store,
        enrich: mpsc::Sender<enrich::Job>,
        ocr: ocr::Handle,
        data_dir: PathBuf,
    ) -> Self {
        // 暂停跨重启：kv `paused` 为 "1" 即暂停；托盘与 bootstrap 都从这个原子读初值
        let paused = matches!(store.kv_get("paused"), Ok(Some(v)) if v == "1");
        Self {
            settings: Mutex::new(settings),
            store: Mutex::new(store),
            enrich,
            ocr,
            data_dir,
            paused: AtomicBool::new(paused),
            last_write_seq: AtomicU64::new(u64::MAX),
            writing: AtomicBool::new(false),
            previous_window: AtomicIsize::new(0),
            onboarding: AtomicBool::new(false),
            dialog_open: AtomicBool::new(false),
            blur_keeps: AtomicBool::new(false),
            shot: Mutex::default(),
            rec: Mutex::default(),
            translating: Mutex::default(),
            tr_downloading: AtomicBool::new(false),
            trcard: Mutex::default(),
        }
    }

    pub fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn settings(&self) -> MutexGuard<'_, Settings> {
        self.settings.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 测试用：内存数据库、无人接收的补全通道、没有识别线程的队列。
    #[cfg(test)]
    pub fn for_test() -> Self {
        let dir = std::env::temp_dir().join(format!("xpaste-app-test-{}", std::process::id()));
        let store = Store::open_in_memory(&dir).expect("内存数据库");
        Self::new(
            Settings::default(),
            store,
            mpsc::channel().0,
            ocr::Handle::new(false),
            dir,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn paused_initial_from_kv() {
        let dir = std::env::temp_dir().join("xpaste-paused-kv-test");
        for (value, want) in [(Some("1"), true), (Some("0"), false), (None, false)] {
            let store = Store::open_in_memory(&dir).expect("内存数据库");
            if let Some(v) = value {
                store.kv_set("paused", v).unwrap();
            }
            let state = AppState::new(
                Settings::default(),
                store,
                mpsc::channel().0,
                crate::ocr::Handle::new(false),
                dir.clone(),
            );
            assert_eq!(state.paused.load(Ordering::SeqCst), want, "{value:?}");
        }
    }
}
