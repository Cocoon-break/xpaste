use std::sync::atomic::Ordering;

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

use crate::error::AppError;
use crate::state::AppState;

/// 录屏只在 macOS 上做（M24）：Windows 上从不注册录屏快捷键。
const REC_SUPPORTED: bool = cfg!(target_os = "macos");

/// 注销全部后注册唤起、截图、录屏、翻译与便签快捷键（后四个为空串表示不启用，跳过；不含晾衣绳的版本不注册便签）；任何失败都视为被占用。
/// 五个都会尝试注册，按唤起、截图、录屏、翻译、便签的顺序返回第一个失败：
/// `HotkeyTaken` / `ShotHotkeyTaken` / `RecHotkeyTaken` / `TranslateHotkeyTaken` / `LineHotkeyTaken`。
pub fn register(
    app: &AppHandle,
    panel: &str,
    shot: &str,
    rec: &str,
    translate: &str,
    line: &str,
) -> Result<(), AppError> {
    let gs = app.global_shortcut();
    let taken = |e: tauri_plugin_global_shortcut::Error| log::debug!("全局快捷键插件错误：{e}");
    let panel_ok = gs
        .unregister_all()
        .and_then(|()| gs.register(panel))
        .map_err(taken)
        .is_ok();
    let shot_ok = shot.is_empty() || gs.register(shot).map_err(taken).is_ok();
    let rec_ok = !REC_SUPPORTED || rec.is_empty() || gs.register(rec).map_err(taken).is_ok();
    let tr_ok = translate.is_empty() || gs.register(translate).map_err(taken).is_ok();
    let line_ok =
        !crate::line::enabled() || line.is_empty() || gs.register(line).map_err(taken).is_ok();
    first_failure(panel_ok, shot_ok, rec_ok, tr_ok, line_ok)
}

/// 按唤起、截图、录屏、翻译、便签的顺序取第一个没注册上的。
fn first_failure(
    panel: bool,
    shot: bool,
    rec: bool,
    translate: bool,
    line: bool,
) -> Result<(), AppError> {
    match (panel, shot, rec, translate, line) {
        (false, ..) => Err(AppError::HotkeyTaken),
        (_, false, ..) => Err(AppError::ShotHotkeyTaken),
        (_, _, false, ..) => Err(AppError::RecHotkeyTaken),
        (_, _, _, false, _) => Err(AppError::TranslateHotkeyTaken),
        (_, _, _, _, false) => Err(AppError::LineHotkeyTaken),
        _ => Ok(()),
    }
}

/// 截图、录屏、翻译、便签快捷键各自是否该注册却没注册上（空串不启用；Windows 上录屏从不注册，不含晾衣绳的版本不注册便签，不算）。
pub fn unregistered(
    app: &AppHandle,
    shot: &str,
    rec: &str,
    translate: &str,
    line: &str,
) -> (bool, bool, bool, bool) {
    let gs = app.global_shortcut();
    let missing = |s: &str| !s.is_empty() && !gs.is_registered(s);
    (
        missing(shot),
        REC_SUPPORTED && missing(rec),
        missing(translate),
        crate::line::enabled() && missing(line),
    )
}

#[derive(Debug, PartialEq, Eq)]
pub enum Role {
    Panel,
    Shot,
    Rec,
    Translate,
    Line,
}

/// 触发的快捷键是哪一个（M23 规则 1、M24、M26 规则 18）：按解析后的组合比较；截图、录屏、翻译为空串时不认。
/// Windows 上录屏不注册、设置里也不校验，它留着的组合可能正是翻译快捷键，不认录屏。
pub fn role_of(
    pressed: &Shortcut,
    panel: &str,
    shot: &str,
    rec: &str,
    translate: &str,
    line: &str,
) -> Option<Role> {
    let is = |s: &str| !s.is_empty() && s.parse::<Shortcut>().is_ok_and(|k| k == *pressed);
    if is(panel) {
        Some(Role::Panel)
    } else if is(shot) {
        Some(Role::Shot)
    } else if REC_SUPPORTED && is(rec) {
        Some(Role::Rec)
    } else if is(translate) {
        Some(Role::Translate)
    } else if crate::line::enabled() && is(line) {
        Some(Role::Line)
    } else {
        None
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ShotRoute {
    /// 交给面板（`hotkey-pressed`，载荷 `"shot"` / `"rec"` / `"translate"`）：录制框聚焦时写入录制框，
    /// 否则由面板调 `overlay_begin`（截图、录屏）或 `trcard_press`（翻译）开始
    Recorder,
    Begin,
}

/// 设置或确认框开着、面板可见且有焦点时先交给面板（M15 规则 3）：网页收不到被全局注册拦下的按键，录制框只能这样拿到。
/// 其余情况直接开始；对话框开着时面板不隐藏（M15 规则 4，见 `shot::begin`）。
pub fn shot_route(dialog_open: bool, panel_visible: bool, panel_focused: bool) -> ShotRoute {
    if dialog_open && panel_visible && panel_focused {
        ShotRoute::Recorder
    } else {
        ShotRoute::Begin
    }
}

/// 截图快捷键按下：按 `shot_route` 通知面板或开始截图。主线程调用。
pub fn shot_pressed(app: &AppHandle) {
    overlay_pressed(app, crate::shot::Mode::Shot);
}

/// 翻译快捷键按下（M26 规则 18）：设置或确认框开着且面板有焦点时同截图交给面板（M15 规则 3：在录制框里就写入、不翻译，
/// 否则面板调 `trcard_press` 照常弹卡片），其余情况直接弹划词翻译卡片。主线程调用。
pub fn translate_pressed(app: &AppHandle) {
    let route = route_now(app);
    log::info!("translate 快捷键按下 {route:?}");
    match route {
        ShotRoute::Recorder => {
            crate::events::emit_to(app, "panel", crate::events::HOTKEY_PRESSED, "translate")
        }
        // 截图、录屏遮罩或长截图进行中不弹卡片：卡片在遮罩下面看不见，却会拿走键盘
        ShotRoute::Begin if crate::shot::active(app) || crate::longshot::active() => {}
        ShotRoute::Begin => crate::trcard::pressed(app),
    }
}

/// 按面板此刻的状态取 `shot_route`。
fn route_now(app: &AppHandle) -> ShotRoute {
    let (visible, focused) = app.get_webview_window("panel").map_or((false, false), |w| {
        (
            w.is_visible().unwrap_or(false),
            w.is_focused().unwrap_or(false),
        )
    });
    let dialog_open = app.state::<AppState>().dialog_open.load(Ordering::SeqCst);
    shot_route(dialog_open, visible, focused)
}

/// 截图或录屏快捷键要开遮罩时：按 `shot_route` 通知面板或开始（遮罩已上屏时退出、冻结中忽略，见 `shot::begin`）。
fn overlay_pressed(app: &AppHandle, mode: crate::shot::Mode) {
    let route = route_now(app);
    log::info!("{} 快捷键按下 {route:?}", mode.as_str());
    match route {
        ShotRoute::Recorder => {
            crate::events::emit_to(app, "panel", crate::events::HOTKEY_PRESSED, mode.as_str())
        }
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
pub fn failure_notice(
    panel_failed: bool,
    shot_failed: bool,
    rec_failed: bool,
    translate_failed: bool,
    line_failed: bool,
) -> Option<String> {
    let names: Vec<_> = [
        (panel_failed, "唤起"),
        (shot_failed, "截图"),
        (rec_failed, "录屏"),
        (translate_failed, "翻译"),
        (line_failed, "便签"),
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
    let key = match key.as_str() {
        "SPACE" => "Space",
        "UP" => "↑",
        "DOWN" => "↓",
        "LEFT" => "←",
        "RIGHT" => "→",
        k => k,
    };
    let parts = [
        (m, if mac { "⌘" } else { "Ctrl" }),
        (c && (mac || !m), if mac { "⌃" } else { "Ctrl" }),
        (a, if mac { "⌥" } else { "Alt" }),
        (s, "Shift"),
        (!key.is_empty(), key),
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
    use super::{
        REC_SUPPORTED, Role, ShotRoute, failure_notice, first_failure, label, role_of, shot_route,
    };
    use crate::error::AppError;

    #[test]
    fn shot_goes_to_panel_only_while_dialog_open_and_panel_focused() {
        use ShotRoute::*;
        assert_eq!(shot_route(true, true, true), Recorder);
        // 面板失焦（切去了别的应用）、对话框没开或面板不可见：照常截图
        assert_eq!(shot_route(true, true, false), Begin);
        assert_eq!(shot_route(false, true, true), Begin);
        assert_eq!(shot_route(false, true, false), Begin);
        assert_eq!(shot_route(false, false, false), Begin);
        assert_eq!(shot_route(true, false, false), Begin);
        assert_eq!(shot_route(true, false, true), Begin);
    }

    #[test]
    fn failure_notice_names_every_taken_hotkey() {
        let tail = "快捷键被其他应用占用，请在设置中更换";
        let n = |p, s, r| failure_notice(p, s, r, false, false);
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
        assert_eq!(
            failure_notice(false, false, false, true, false),
            Some(format!("翻译{tail}"))
        );
        assert_eq!(
            failure_notice(true, true, true, true, false),
            Some(format!("唤起、截图、录屏与翻译{tail}"))
        );
        assert_eq!(
            failure_notice(false, false, false, false, true),
            Some(format!("便签{tail}"))
        );
        assert_eq!(
            failure_notice(true, true, true, true, true),
            Some(format!("唤起、截图、录屏、翻译与便签{tail}"))
        );
    }

    #[test]
    fn role_of_dispatches() {
        let shot = "Alt+Shift+A".parse().unwrap();
        let panel = "CommandOrControl+Shift+V".parse().unwrap();
        let rec = "Alt+Shift+R".parse().unwrap();
        let (p, s, r) = ("CommandOrControl+Shift+V", "Alt+Shift+A", "Alt+Shift+R");
        let t = "Alt+Shift+T";
        let l = "Alt+Shift+N";
        assert_eq!(role_of(&shot, p, s, r, t, l), Some(Role::Shot));
        assert_eq!(role_of(&panel, p, s, r, t, l), Some(Role::Panel));
        // Windows 不注册录屏快捷键（REC_SUPPORTED），也就不认
        assert_eq!(
            role_of(&rec, p, s, r, t, l),
            REC_SUPPORTED.then_some(Role::Rec)
        );
        // 截图、录屏为空串表示不启用：不认
        assert_eq!(role_of(&shot, p, "", r, t, l), None);
        assert_eq!(role_of(&rec, p, s, "", t, l), None);
        assert_eq!(role_of(&panel, p, "", "", "", ""), Some(Role::Panel));
    }

    #[test]
    fn role_of_translate() {
        let tr = "Alt+Shift+T".parse().unwrap();
        let (p, s, r) = ("CommandOrControl+Shift+V", "Alt+Shift+A", "Alt+Shift+R");
        assert_eq!(
            role_of(&tr, p, s, r, "alt+shift+t", ""),
            Some(Role::Translate)
        );
        // 为空串表示不启用：不认
        assert_eq!(role_of(&tr, p, s, r, "", ""), None);
        // Windows 上录屏不注册（M24）：翻译用了留着默认值的录屏组合时按翻译认，不能开录屏
        let rec = "Alt+Shift+R".parse().unwrap();
        let want = if cfg!(target_os = "macos") {
            Role::Rec
        } else {
            Role::Translate
        };
        assert_eq!(role_of(&rec, p, s, r, r, ""), Some(want));
    }

    #[test]
    fn role_of_line() {
        let key = "Alt+Shift+N".parse().unwrap();
        let (p, s, r, t) = (
            "CommandOrControl+Shift+V",
            "Alt+Shift+A",
            "Alt+Shift+R",
            "Alt+Shift+T",
        );
        // 不含晾衣绳的版本不注册便签，也就不认
        assert_eq!(
            role_of(&key, p, s, r, t, "alt+shift+n"),
            crate::line::enabled().then_some(Role::Line)
        );
        // 为空串表示不启用：不认
        assert_eq!(role_of(&key, p, s, r, t, ""), None);
    }

    #[test]
    fn register_reports_first_failure_in_order() {
        let kind = |f: [bool; 5]| match first_failure(f[0], f[1], f[2], f[3], f[4]) {
            Ok(()) => "ok",
            Err(AppError::HotkeyTaken) => "panel",
            Err(AppError::ShotHotkeyTaken) => "shot",
            Err(AppError::RecHotkeyTaken) => "rec",
            Err(AppError::TranslateHotkeyTaken) => "translate",
            Err(AppError::LineHotkeyTaken) => "line",
            Err(_) => "other",
        };
        assert_eq!(kind([true; 5]), "ok");
        assert_eq!(kind([true, true, true, true, false]), "line");
        assert_eq!(kind([true, true, true, false, false]), "translate");
        assert_eq!(kind([true, true, false, false, false]), "rec");
        assert_eq!(kind([true, false, false, false, false]), "shot");
        assert_eq!(kind([false; 5]), "panel");
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
        assert_eq!(
            label("Alt+Space"),
            if mac { "⌥ + Space" } else { "Alt + Space" }
        );
        assert_eq!(label("Alt+`"), if mac { "⌥ + `" } else { "Alt + `" });
        assert_eq!(label("Alt+Up"), if mac { "⌥ + ↑" } else { "Alt + ↑" });
    }

    #[test]
    fn symbol_and_space_keys_parse() {
        for k in [
            "CommandOrControl+`",
            "CommandOrControl+Shift+Space",
            "Alt+;",
            "Alt+\\",
            "Alt+Left",
            "Alt+F20",
        ] {
            assert!(
                k.parse::<tauri_plugin_global_shortcut::Shortcut>().is_ok(),
                "{k}"
            );
        }
    }
}
