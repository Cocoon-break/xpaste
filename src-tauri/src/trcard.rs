//! 划词翻译卡片（设计 §3.2）。

use tauri::AppHandle;

/// 翻译快捷键按下、不交给面板录制框时调用。主线程调用。
// ponytail: 先只记日志，Task 6 换成真正的卡片
pub fn pressed(_app: &AppHandle) {
    log::info!("trcard pressed");
}
