//! 表情包制作：总开关、快捷键与托盘入口的落点（只在带 Pro 的构建里启用）。

use tauri::AppHandle;

/// 表情包制作是否编入此构建（与搅拌机同一个条件）。设置里的总开关由调用方另查。
pub fn enabled() -> bool {
    crate::line::enabled()
}

/// 读入设置、保存设置时整理风格库：去重、去掉坏项（规则在 Pro 里，不含 Pro 的构建不动）。
pub fn normalize_settings(settings: &mut xpaste_core::Settings) {
    if enabled() {
        let (used, custom) = xpaste_pro::meme_normalize_styles(
            std::mem::take(&mut settings.meme_styles_used),
            std::mem::take(&mut settings.meme_styles_custom),
        );
        settings.meme_styles_used = used;
        settings.meme_styles_custom = custom;
    }
}

/// 表情包快捷键按下（主线程）：面板在屏上且拿着键盘时交给面板（设置里的录制框要接收这个组合）；
/// 否则打开空白的制作窗口，窗口在下一步接上，现在什么也不做。
pub fn hotkey_pressed(app: &AppHandle) {
    if crate::panel::visible(app) && crate::panel::holds_key(app) {
        crate::events::emit_to(app, "panel", crate::events::HOTKEY_PRESSED, "meme");
        return;
    }
    open(app, &[]);
}

/// 带着这些记录打开制作窗口（`ids` 为空 = 空白打开；托盘、面板入口调用）。制作窗口在下一步接上，现在什么也不做。
pub fn open(_app: &AppHandle, _ids: &[i64]) {}
