//! 传送带运行时：按顺序一条条取用的粘贴队列。

/// 传送带是否可用：与晾衣绳同一个构建条件。
pub fn enabled() -> bool {
    crate::line::enabled()
}

/// 传送带快捷键按下。
pub fn hotkey_pressed(_app: &tauri::AppHandle) {
    log::info!("传送带快捷键");
}
