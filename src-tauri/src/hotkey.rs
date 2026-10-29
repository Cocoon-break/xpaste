use std::sync::atomic::Ordering;

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

use crate::error::AppError;
use crate::state::AppState;

/// 录屏只在 macOS 上做（M24）：Windows 上从不注册录屏快捷键。
const REC_SUPPORTED: bool = cfg!(target_os = "macos");

/// 注销全部后注册唤起、截图与录屏快捷键（截图、录屏为空串表示不启用，跳过）；任何失败都视为被占用。
/// 三个都会尝试注册，按唤起、截图、录屏的顺序返回第一个失败：`HotkeyTaken` / `ShotHotkeyTaken` / `RecHotkeyTaken`。
pub fn register(app: &AppHandle, panel: &str, shot: &str, rec: &str) -> Result<(), AppError> {
    let gs = app.global_shortcut();
    let taken = |e: tauri_plugin_global_shortcut::Error| log::debug!("全局快捷键插件错误：{e}");
    let panel_ok = gs
        .unregister_all()
        .and_then(|()| gs.register(panel))
        .map_err(taken)
        .is_ok();
    let shot_ok = shot.is_empty() || gs.register(shot).map_err(taken).is_ok();
    let rec_ok = !REC_SUPPORTED || rec.is_empty() || gs.register(rec).map_err(taken).is_ok();
    match (panel_ok, shot_ok, rec_ok) {
        (false, ..) => Err(AppError::HotkeyTaken),
        (true, false, _) => Err(AppError::ShotHotkeyTaken),
        (true, true, false) => Err(AppError::RecHotkeyTaken),
        (true, true, true) => Ok(()),
    }
}

/// 截图、录屏快捷键各自是否该注册却没注册上（空串不启用；Windows 上录屏从不注册，不算）。
pub fn unregistered(app: &AppHandle, shot: &str, rec: &str) -> (bool, bool) {
    let gs = app.global_shortcut();
    let missing = |s: &str| !s.is_empty() && !gs.is_registered(s);
    (missing(shot), REC_SUPPORTED && missing(rec))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Role {
    Panel,
    Shot,
    Rec,
}

/// 触发的快捷键是哪一个（M23 规则 1、M24）：按解析后的组合比较；截图、录屏为空串时不认。
pub fn role_of(pressed: &Shortcut, panel: &str, shot: &str, rec: &str) -> Option<Role> {
    let is = |s: &str| !s.is_empty() && s.parse::<Shortcut>().is_ok_and(|k| k == *pressed);
    if is(panel) {
        Some(Role::Panel)
    } else if is(shot) {
        Some(Role::Shot)
    } else if is(rec) {
        Some(Role::Rec)
    } else {
        None
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ShotRoute {
    /// 交给录制框（`hotkey-pressed`，载荷 `"shot"` / `"rec"`）
    Recorder,
    /// 只把面板拉回焦点：开始截图会隐藏面板、关掉对话框，未保存的修改就丢了（M15 规则 4）
    Refocus,
    Begin,
}

/// 设置或确认框开着且面板可见时从不开始截图：有焦点交给录制框（同 M15 规则 3），失焦只取回焦点（同 M15 规则 4）。
pub fn shot_route(dialog_open: bool, panel_visible: bool, panel_focused: bool) -> ShotRoute {
    match (dialog_open && panel_visible, panel_focused) {
        (false, _) => ShotRoute::Begin,
        (true, true) => ShotRoute::Recorder,
        (true, false) => ShotRoute::Refocus,
    }
}

/// 截图快捷键按下：按 `shot_route` 通知面板、取回面板焦点或开始截图。主线程调用。
pub fn shot_pressed(app: &AppHandle) {
    overlay_pressed(app, crate::shot::Mode::Shot);
}

/// 截图或录屏快捷键要开遮罩时：按 `shot_route` 通知面板、取回面板焦点或开始（遮罩已上屏时退出、冻结中忽略，见 `shot::begin`）。
fn overlay_pressed(app: &AppHandle, mode: crate::shot::Mode) {
    let (visible, focused) = app.get_webview_window("panel").map_or((false, false), |w| {
        (
            w.is_visible().unwrap_or(false),
            w.is_focused().unwrap_or(false),
        )
    });
    let dialog_open = app.state::<AppState>().dialog_open.load(Ordering::SeqCst);
    let route = shot_route(dialog_open, visible, focused);
    log::info!("{} 快捷键按下 {route:?}", mode.as_str());
    match route {
        ShotRoute::Recorder => {
            crate::events::emit_to(app, "panel", crate::events::HOTKEY_PRESSED, mode.as_str())
        }
        ShotRoute::Refocus => crate::panel::refocus(app),
        ShotRoute::Begin => crate::shot::begin(app, mode),
    }
}

/// 录屏快捷键按下（M24 规则 1）：录制中（含开始中）停止，停止写文件期间忽略，空闲时按截图的规则开遮罩。主线程调用。
pub fn rec_pressed(app: &AppHandle) {
    use crate::rec::Phase;
    match crate::rec::phase(app) {
        Phase::Recording => {
            log::info!("rec 快捷键按下，停止录制");
            crate::rec::stop(app);
        }
        Phase::Stopping => log::info!("rec 快捷键按下，正在停止，忽略"),
        // 遮罩已上屏时再按退出、冻结中忽略，交给 shot_route 与 Session::try_begin_at
        Phase::Idle => overlay_pressed(app, crate::shot::Mode::Rec),
    }
}

/// 启动时注册失败的系统通知文案：被占用的几个一起说，“、”连接，最后两个用“与”。
pub fn failure_notice(panel_failed: bool, shot_failed: bool, rec_failed: bool) -> Option<String> {
    let names: Vec<_> = [
        (panel_failed, "唤起"),
        (shot_failed, "截图"),
        (rec_failed, "录屏"),
    ]
    .into_iter()
    .filter_map(|(failed, name)| failed.then_some(name))
    .collect();
    let (last, rest) = names.split_last()?;
    let names = match rest {
        [] => (*last).to_string(),
        _ => format!("{}与{last}", rest.join("、")),
    };
    Some(format!("{names}快捷键被其他应用占用，请在设置中更换"))
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
    use super::{Role, ShotRoute, failure_notice, label, role_of, shot_route};

    #[test]
    fn shot_never_begins_while_dialog_open_and_panel_visible() {
        use ShotRoute::*;
        assert_eq!(shot_route(true, true, true), Recorder);
        assert_eq!(shot_route(true, true, false), Refocus);
        // 对话框没开或面板不可见：照常截图
        assert_eq!(shot_route(false, true, true), Begin);
        assert_eq!(shot_route(false, true, false), Begin);
        assert_eq!(shot_route(false, false, false), Begin);
        assert_eq!(shot_route(true, false, false), Begin);
        assert_eq!(shot_route(true, false, true), Begin);
    }

    #[test]
    fn failure_notice_names_every_taken_hotkey() {
        let tail = "快捷键被其他应用占用，请在设置中更换";
        let n = |p, s, r| failure_notice(p, s, r);
        assert_eq!(n(false, false, false), None);
        for (flags, names) in [
            ((true, false, false), "唤起"),
            ((false, true, false), "截图"),
            ((false, false, true), "录屏"),
            ((true, true, false), "唤起与截图"),
            ((true, false, true), "唤起与录屏"),
            ((false, true, true), "截图与录屏"),
            ((true, true, true), "唤起、截图与录屏"),
        ] {
            assert_eq!(n(flags.0, flags.1, flags.2), Some(format!("{names}{tail}")));
        }
    }

    #[test]
    fn role_of_dispatches() {
        let shot = "Alt+Shift+A".parse().unwrap();
        let panel = "CommandOrControl+Shift+V".parse().unwrap();
        let rec = "Alt+Shift+R".parse().unwrap();
        let (p, s, r) = ("CommandOrControl+Shift+V", "Alt+Shift+A", "Alt+Shift+R");
        assert_eq!(role_of(&shot, p, s, r), Some(Role::Shot));
        assert_eq!(role_of(&panel, p, s, r), Some(Role::Panel));
        assert_eq!(role_of(&rec, p, s, r), Some(Role::Rec));
        // 截图、录屏为空串表示不启用：不认
        assert_eq!(role_of(&shot, p, "", r), None);
        assert_eq!(role_of(&rec, p, s, ""), None);
        assert_eq!(role_of(&panel, p, "", ""), Some(Role::Panel));
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
