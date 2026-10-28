use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

use crate::error::AppError;

/// 注销全部后注册唤起与截图两个快捷键（截图为空串表示不启用，跳过）；任何失败都视为被占用。
/// 两个都会尝试注册，唤起失败返回 `HotkeyTaken`，否则截图失败返回 `ShotHotkeyTaken`。
pub fn register(app: &AppHandle, panel: &str, shot: &str) -> Result<(), AppError> {
    let gs = app.global_shortcut();
    let taken = |e: tauri_plugin_global_shortcut::Error| log::debug!("全局快捷键插件错误：{e}");
    let panel_ok = gs
        .unregister_all()
        .and_then(|()| gs.register(panel))
        .map_err(taken)
        .is_ok();
    let shot_ok = shot.is_empty() || gs.register(shot).map_err(taken).is_ok();
    match (panel_ok, shot_ok) {
        (false, _) => Err(AppError::HotkeyTaken),
        (true, false) => Err(AppError::ShotHotkeyTaken),
        (true, true) => Ok(()),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Role {
    Panel,
    Shot,
}

/// 触发的快捷键是哪一个（设计 §4.7）：按解析后的组合比较；截图为空串时只认唤起。
pub fn role_of(pressed: &Shortcut, panel: &str, shot: &str) -> Option<Role> {
    let is = |s: &str| !s.is_empty() && s.parse::<Shortcut>().is_ok_and(|k| k == *pressed);
    if is(panel) {
        Some(Role::Panel)
    } else if is(shot) {
        Some(Role::Shot)
    } else {
        None
    }
}

/// 快捷键字符串 → 显示标签（如 `⌘ + Shift + V`）；规则与前端 `hotkeyLabel` 一致。
pub fn label(shortcut: &str) -> String {
    let mac = cfg!(target_os = "macos");
    let (mut m, mut c, mut a, mut s, mut key) = (false, false, false, false, String::new());
    for t in shortcut.split('+').map(|t| t.trim().to_ascii_lowercase()) {
        match t.as_str() {
            "commandorcontrol" | "cmdorctrl" | "command" | "cmd" | "super" => m = true,
            // 字面的 Control 键：macOS 上是 ⌃，和 ⌘ 不同；Windows 上就是 Ctrl，与 CommandOrControl 合并显示
            "control" | "ctrl" => c = true,
            "alt" => a = true,
            "shift" => s = true,
            "" => {}
            _ => key = t.to_ascii_uppercase(),
        }
    }
    let parts = [
        (m, if mac { "⌘" } else { "Ctrl" }),
        (c && (mac || !m), if mac { "⌃" } else { "Ctrl" }),
        (a, if mac { "⌥" } else { "Alt" }),
        (s, "Shift"),
        (!key.is_empty(), key.as_str()),
    ];
    parts
        .iter()
        .filter(|p| p.0)
        .map(|p| p.1)
        .collect::<Vec<_>>()
        .join(" + ")
}

#[cfg(test)]
mod tests {
    use super::{Role, label, role_of};

    #[test]
    fn role_of_dispatches() {
        let shot = "Alt+Shift+A".parse().unwrap();
        let panel = "CommandOrControl+Shift+V".parse().unwrap();
        let (p, s) = ("CommandOrControl+Shift+V", "Alt+Shift+A");
        assert_eq!(role_of(&shot, p, s), Some(Role::Shot));
        assert_eq!(role_of(&panel, p, s), Some(Role::Panel));
        // 截图为空串表示不启用：只认唤起
        assert_eq!(role_of(&shot, p, ""), None);
        assert_eq!(role_of(&panel, p, ""), Some(Role::Panel));
    }

    #[test]
    fn label_follows_platform() {
        let mac = cfg!(target_os = "macos");
        assert_eq!(
            label("CommandOrControl+Shift+V"),
            if mac {
                "⌘ + Shift + V"
            } else {
                "Ctrl + Shift + V"
            }
        );
        assert_eq!(label("Alt+F2"), if mac { "⌥ + F2" } else { "Alt + F2" });
        assert_eq!(label("Control+Z"), if mac { "⌃ + Z" } else { "Ctrl + Z" });
        assert_eq!(
            label("CommandOrControl+Control+Z"),
            if mac { "⌘ + ⌃ + Z" } else { "Ctrl + Z" }
        );
    }
}
