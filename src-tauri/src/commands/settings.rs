use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use xpaste_core::{RecordSummary, Settings};

use crate::clock::now_ms;
use crate::error::AppError;
use crate::events::{PAUSED_CHANGED, emit, emit_removed};
use crate::state::AppState;
use tauri_plugin_autostart::ManagerExt;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    pub os: &'static str,
    pub material: &'static str,
    /// macOS 主版本号；Windows 为 `None`。设置页据此禁用“录制麦克风”（macOS 15 起，M24）
    pub macos_major: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    pub settings: Settings,
    pub platform: PlatformInfo,
    pub records: Vec<RecordSummary>,
    pub first_run: bool,
    pub paused: bool,
}

pub fn bootstrap_inner(state: &AppState) -> Result<Bootstrap, AppError> {
    let (records, first_run) = {
        let store = state.store();
        let records: Vec<_> = store
            .list_summaries()?
            .into_iter()
            .map(crate::assets::summary_dto)
            .collect();
        (records, store.kv_get("onboarded")?.is_none())
    };
    log::info!("bootstrap 返回 {} 条记录", records.len());
    Ok(Bootstrap {
        settings: state.settings().clone(),
        platform: PlatformInfo {
            os: xpaste_platform::OS,
            material: crate::panel::material(),
            macos_major: xpaste_platform::macos_version(),
        },
        records,
        first_run,
        paused: state.paused.load(Ordering::SeqCst),
    })
}

#[tauri::command(async)]
pub fn bootstrap(state: State<'_, AppState>) -> Result<Bootstrap, AppError> {
    bootstrap_inner(&state)
}

/// 校验、持久化并生效新设置。不处理快捷键、不清理、不发事件。
pub fn save_settings_inner(state: &AppState, mut s: Settings) -> Result<(), AppError> {
    s.normalize();
    s.validate()?;
    let mut cur = state.settings();
    // 面板尺寸只由 panel::remember_size 写：持锁取当前值，不用调用方早先的快照
    s.panel_size = cur.panel_size;
    state.store().save_settings(&s)?;
    *cur = s;
    Ok(())
}

/// 按当前设置的保留时长与条数上限清理，返回已删除的 id。设置已生效，失败只记日志。
pub fn prune_to_settings(state: &AppState) -> Vec<i64> {
    let (days, max) = {
        let s = state.settings();
        (s.retention_days, s.max_records)
    };
    let mut store = state.store();
    let mut removed = store.prune_retention(days, now_ms()).unwrap_or_else(|e| {
        log::warn!("保存设置后保留时长清理失败：{e}");
        Vec::new()
    });
    match store.prune_capacity(max) {
        Ok(ids) => removed.extend(ids),
        Err(e) => log::warn!("保存设置后条数上限清理失败：{e}"),
    }
    removed
}

/// 与旧设置不同的字段名（camelCase，与 kv.settings 一致），供日志与生效判断；不含字段值。
pub fn changed_fields(old: &Settings, new: &Settings) -> Vec<&'static str> {
    [
        ("hotkey", old.hotkey != new.hotkey),
        ("retentionDays", old.retention_days != new.retention_days),
        ("maxRecords", old.max_records != new.max_records),
        ("ignoredApps", old.ignored_apps != new.ignored_apps),
        ("skipSensitive", old.skip_sensitive != new.skip_sensitive),
        ("moveToTop", old.move_to_top != new.move_to_top),
        ("launchAtLogin", old.launch_at_login != new.launch_at_login),
        ("appearance", old.appearance != new.appearance),
        ("panelSize", old.panel_size != new.panel_size),
        ("lastFilter", old.last_filter != new.last_filter),
        (
            "spectrumCollapsed",
            old.spectrum_collapsed != new.spectrum_collapsed,
        ),
        (
            "frequentMinUses",
            old.frequent_min_uses != new.frequent_min_uses,
        ),
        ("ocrEnabled", old.ocr_enabled != new.ocr_enabled),
        ("ocrBackfill", old.ocr_backfill != new.ocr_backfill),
        (
            "searchFileText",
            old.search_file_text != new.search_file_text,
        ),
        ("shotHotkey", old.shot_hotkey != new.shot_hotkey),
        ("shotDir", old.shot_dir != new.shot_dir),
        ("shotSaveCopy", old.shot_save_copy != new.shot_save_copy),
        ("recHotkey", old.rec_hotkey != new.rec_hotkey),
        ("recDir", old.rec_dir != new.rec_dir),
        (
            "recSystemAudio",
            old.rec_system_audio != new.rec_system_audio,
        ),
        ("recMicrophone", old.rec_microphone != new.rec_microphone),
        ("recCopy", old.rec_copy != new.rec_copy),
        ("recQuality", old.rec_quality != new.rec_quality),
        ("recFps", old.rec_fps != new.rec_fps),
        (
            "translateEngine",
            old.translate_engine != new.translate_engine,
        ),
        (
            "translateBaseUrl",
            old.translate_base_url != new.translate_base_url,
        ),
        (
            "translateApiKey",
            old.translate_api_key != new.translate_api_key,
        ),
        ("translateModel", old.translate_model != new.translate_model),
        ("translateLang", old.translate_lang != new.translate_lang),
    ]
    .into_iter()
    .filter_map(|(name, changed)| changed.then_some(name))
    .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum HotkeyAction {
    /// 唤起、截图或录屏快捷键改了：先注册新的三个，失败则恢复旧的三个并返回 `HotkeyTaken` / `ShotHotkeyTaken` / `RecHotkeyTaken`，不保存
    Register,
    /// 没改但唤起快捷键启动时注册失败过，或截图、录屏快捷键没注册上：保存之后顺带重试，失败不算保存失败
    RetryBestEffort,
    None,
}

/// 启动时注册失败而组合仍被占用时，不能让每次保存（分类防抖、引导、改其他字段）都因重试失败而保存不了。
/// `others_unregistered`：截图或录屏快捷键非空却没注册上（托盘只记唤起的失败）。
pub fn hotkey_action(
    hotkey_changed: bool,
    startup_failed: bool,
    others_unregistered: bool,
) -> HotkeyAction {
    match (hotkey_changed, startup_failed || others_unregistered) {
        (true, _) => HotkeyAction::Register,
        (false, true) => HotkeyAction::RetryBestEffort,
        (false, false) => HotkeyAction::None,
    }
}

fn hotkeys_changed(changed: &[&str]) -> bool {
    ["hotkey", "shotHotkey", "recHotkey"]
        .iter()
        .any(|k| changed.contains(k))
}

/// 开关开机启动；开发构建从不改系统登录项。失败只记日志（错误里可能带路径，只在 debug 级别输出）。
fn set_autostart(app: &AppHandle, on: bool) {
    if cfg!(debug_assertions) {
        log::info!("开发构建不改开机启动项");
        return;
    }
    let al = app.autolaunch();
    let r = if on { al.enable() } else { al.disable() };
    if let Err(e) = r {
        log::warn!("{}开机启动失败", if on { "开启" } else { "关闭" });
        log::debug!("开机启动插件错误：{e}");
    }
}

/// 校验 → 快捷键改了就注册新的唤起、截图与录屏快捷键（失败恢复旧的三个）→ 持久化（失败也恢复）；返回变化的字段名。
/// `register(唤起, 截图, 录屏)` 是唯一注入点（正式为 `hotkey::register`），测试借此模拟快捷键被占用。
pub fn commit_settings(
    state: &AppState,
    mut settings: Settings,
    mut register: impl FnMut(&str, &str, &str) -> Result<(), AppError>,
) -> Result<Vec<&'static str>, AppError> {
    // 先去空白再校验、比较：粘贴带空格的 Base URL 不该报错，也不该算作改动（M26 规则 6）
    settings.normalize();
    settings.validate()?;
    let old = state.settings().clone();
    // 面板尺寸只由拖动记忆（panel::remember_size）写；这里只为 changed 不含 panelSize，落库时 save_settings_inner 持锁再取
    settings.panel_size = old.panel_size;
    let changed = changed_fields(&old, &settings);
    let hotkey_changed = hotkeys_changed(&changed);
    // register 会先注销全部快捷键，失败时要把旧的三个注册回去
    let restore = |register: &mut dyn FnMut(&str, &str, &str) -> Result<(), AppError>| {
        // 截图、录屏键在被占用时恢复不了很常见（多半正是它被占用），只记 debug
        match register(&old.hotkey, &old.shot_hotkey, &old.rec_hotkey) {
            Err(e @ AppError::HotkeyTaken) => log::warn!("恢复原快捷键失败：{e}"),
            Err(e) => log::debug!("恢复原截图或录屏快捷键失败：{e}"),
            Ok(()) => {}
        }
    };
    if hotkey_changed
        && let Err(e) = register(
            &settings.hotkey,
            &settings.shot_hotkey,
            &settings.rec_hotkey,
        )
    {
        restore(&mut register);
        return Err(e);
    }
    if let Err(e) = save_settings_inner(state, settings) {
        if hotkey_changed {
            restore(&mut register);
        }
        return Err(e);
    }
    Ok(changed)
}

#[tauri::command(async)]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<(), AppError> {
    let (hotkey, shot_hotkey, rec_hotkey, appearance, launch, ocr_on) = (
        settings.hotkey.clone(),
        settings.shot_hotkey.clone(),
        settings.rec_hotkey.clone(),
        settings.appearance,
        settings.launch_at_login,
        settings.ocr_enabled,
    );
    let changed = commit_settings(&state, settings, |p, s, r| {
        crate::hotkey::register(&app, p, s, r)
    })?;
    // 快捷键改了必为 Register，与 commit_settings 内一致；没改时只决定是否尽力重试
    let (shot_missing, rec_missing) = crate::hotkey::unregistered(&app, &shot_hotkey, &rec_hotkey);
    let action = hotkey_action(
        hotkeys_changed(&changed),
        crate::tray::hotkey_failed(),
        shot_missing || rec_missing,
    );
    log::info!("设置已保存 changed=[{}]", changed.join(", "));
    // 设置已持久化并生效：以下步骤失败不回滚、不算保存失败（都只记日志）
    let removed = prune_to_settings(&state);
    emit_removed(&app, &removed);
    if changed.contains(&"appearance") {
        crate::panel::apply_theme(&app, appearance);
    }
    match action {
        HotkeyAction::Register => crate::tray::set_hotkey_label(&hotkey),
        // 唤起注册上后 set_hotkey_label 清掉失败状态（截图、录屏失败不算）；唤起失败时原本也没注册上，无需恢复
        HotkeyAction::RetryBestEffort => {
            if !matches!(
                crate::hotkey::register(&app, &hotkey, &shot_hotkey, &rec_hotkey),
                Err(AppError::HotkeyTaken)
            ) {
                crate::tray::set_hotkey_label(&hotkey);
            }
        }
        HotkeyAction::None => {}
    }
    if changed.contains(&"launchAtLogin") {
        set_autostart(&app, launch);
    }
    // 回填线程自己再看 ocrEnabled 与 ocrBackfill 是否都开着
    if changed.contains(&"ocrEnabled") || changed.contains(&"ocrBackfill") {
        if ocr_on {
            state.ocr.schedule_backfill(&app, Duration::ZERO);
        } else {
            state.ocr.disable(&app);
        }
    }
    Ok(())
}

/// 按给定规则会清理掉的非置顶记录数（设置页确认用）。
#[tauri::command(async)]
pub fn prune_preview(
    state: State<'_, AppState>,
    retention_days: Option<u32>,
    max_records: u32,
) -> Result<u32, AppError> {
    Ok(state
        .store()
        .prune_preview(retention_days, max_records, now_ms())?)
}

/// 命令与托盘菜单共用：写暂停标志、记入 kv（跨重启）、更新托盘、通知前端。
pub fn set_paused_inner(app: &AppHandle, on: bool) {
    let state = app.state::<AppState>();
    state.paused.store(on, Ordering::SeqCst);
    // 临时 guard 在语句末释放，托盘与事件不在锁内
    if let Err(e) = state.store().kv_set("paused", if on { "1" } else { "0" }) {
        log::warn!("记录暂停状态失败：{e}");
    }
    crate::tray::set_paused(app, on);
    emit(app, PAUSED_CHANGED, on);
}

#[tauri::command(async)]
pub fn set_paused(app: AppHandle, on: bool) -> Result<(), AppError> {
    set_paused_inner(&app, on);
    Ok(())
}

/// 引导结束：记下已引导、恢复快捷键切换；按设置兜底开启开机启动（前端已先 `save_settings`）。
#[tauri::command(async)]
pub fn finish_onboarding(app: AppHandle) -> Result<(), AppError> {
    let state = app.state::<AppState>();
    // 先恢复快捷键切换：记 kv 失败也不能让本次运行的快捷键收不起面板
    state.onboarding.store(false, Ordering::SeqCst);
    state.store().kv_set("onboarded", "1")?;
    let launch = state.settings().launch_at_login;
    if launch {
        set_autostart(&app, true);
    }
    Ok(())
}

/// 设置里“不记录这些应用”的候选（M15 P1）。日志只记条数，不记名称
#[tauri::command(async)]
pub fn installed_apps() -> Result<Vec<String>, AppError> {
    let apps = xpaste_platform::installed_apps();
    log::info!("应用列表 {} 项", apps.len());
    Ok(apps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xpaste_core::CoreError;

    #[test]
    fn bootstrap_returns_defaults_and_platform() {
        let state = AppState::for_test();
        let b = bootstrap_inner(&state).unwrap();
        assert_eq!(b.settings.max_records, 500);
        assert_eq!(b.platform.os, xpaste_platform::OS);
        assert!(matches!(b.platform.material, "glass" | "opaque"));
        assert!(b.records.is_empty());
        assert!(b.first_run);
        assert!(!b.paused);
    }

    #[test]
    fn bootstrap_reports_paused_flag() {
        let state = AppState::for_test();
        state.paused.store(true, Ordering::SeqCst);
        assert!(bootstrap_inner(&state).unwrap().paused);
    }

    #[test]
    fn save_settings_inner_rejects_invalid() {
        let state = AppState::for_test();
        let bad = Settings {
            max_records: 33,
            ..Settings::default()
        };
        let r = save_settings_inner(&state, bad);
        assert!(matches!(
            r,
            Err(AppError::Core(CoreError::InvalidSettings(_)))
        ));
        assert_eq!(*state.settings(), Settings::default());
        assert_eq!(state.store().load_settings(), Settings::default());
    }

    #[test]
    fn save_settings_inner_checks_frequent_min_uses_range() {
        let state = AppState::for_test();
        let with = |n| Settings {
            frequent_min_uses: n,
            ..Settings::default()
        };
        for bad in [1, 100] {
            assert!(matches!(
                save_settings_inner(&state, with(bad)),
                Err(AppError::Core(CoreError::InvalidSettings(_)))
            ));
        }
        assert_eq!(*state.settings(), Settings::default());
        for ok in [2, 99] {
            save_settings_inner(&state, with(ok)).unwrap();
            assert_eq!(state.store().load_settings().frequent_min_uses, ok);
        }
    }

    #[test]
    fn save_settings_inner_persists_valid() {
        let state = AppState::for_test();
        let s = Settings {
            max_records: 1000,
            ..Settings::default()
        };
        save_settings_inner(&state, s.clone()).unwrap();
        assert_eq!(*state.settings(), s);
        assert_eq!(state.store().load_settings(), s);
        assert!(prune_to_settings(&state).is_empty());
    }

    #[test]
    fn save_settings_inner_keeps_live_panel_size() {
        let state = AppState::for_test();
        state.settings().panel_size = (1000.0, 700.0);
        save_settings_inner(&state, Settings::default()).unwrap();
        assert_eq!(state.settings().panel_size, (1000.0, 700.0));
        assert_eq!(state.store().load_settings().panel_size, (1000.0, 700.0));
    }

    #[test]
    fn commit_settings_trims_translate_fields_before_validate() {
        let state = AppState::for_test();
        let new = Settings {
            translate_base_url: " https://x/v1/ ".into(),
            translate_model: " m ".into(),
            ..Settings::default()
        };
        let changed = commit_settings(&state, new, |_, _, _| Ok(())).unwrap();
        assert_eq!(changed, ["translateBaseUrl", "translateModel"]);
        let saved = state.store().load_settings();
        assert_eq!(
            (
                saved.translate_base_url.as_str(),
                saved.translate_model.as_str()
            ),
            ("https://x/v1", "m")
        );
        // 只差空白与末尾斜杠，不算改动
        let same = Settings {
            translate_base_url: "https://x/v1///".into(),
            ..saved
        };
        assert!(
            commit_settings(&state, same, |_, _, _| Ok(()))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn changed_fields_lists_only_differences() {
        let old = Settings::default();
        assert!(changed_fields(&old, &old).is_empty());
        let new = Settings {
            hotkey: "Alt+Space".into(),
            appearance: xpaste_core::Appearance::Light,
            panel_size: (1000.0, 700.0),
            spectrum_collapsed: true,
            frequent_min_uses: 5,
            ocr_enabled: false,
            ocr_backfill: false,
            search_file_text: true,
            shot_hotkey: "Alt+Shift+S".into(),
            shot_dir: "/x".into(),
            shot_save_copy: false,
            rec_hotkey: "Alt+Shift+T".into(),
            rec_dir: "/y".into(),
            rec_system_audio: true,
            rec_microphone: true,
            rec_copy: false,
            rec_quality: xpaste_core::RecQuality::High,
            rec_fps: 60,
            ..Settings::default()
        };
        assert_eq!(
            changed_fields(&old, &new),
            [
                "hotkey",
                "appearance",
                "panelSize",
                "spectrumCollapsed",
                "frequentMinUses",
                "ocrEnabled",
                "ocrBackfill",
                "searchFileText",
                "shotHotkey",
                "shotDir",
                "shotSaveCopy",
                "recHotkey",
                "recDir",
                "recSystemAudio",
                "recMicrophone",
                "recCopy",
                "recQuality",
                "recFps"
            ]
        );
    }

    #[test]
    fn hotkey_action_retries_only_best_effort_when_unchanged() {
        assert_eq!(hotkey_action(true, false, false), HotkeyAction::Register);
        assert_eq!(hotkey_action(true, true, true), HotkeyAction::Register);
        assert_eq!(
            hotkey_action(false, true, false),
            HotkeyAction::RetryBestEffort
        );
        // 截图快捷键启动时没注册上（托盘不记）也重试
        assert_eq!(
            hotkey_action(false, false, true),
            HotkeyAction::RetryBestEffort
        );
        assert_eq!(hotkey_action(false, false, false), HotkeyAction::None);
    }

    #[test]
    fn hotkey_taken_restores_old_hotkey_and_keeps_settings() {
        let state = AppState::for_test();
        let old = state.settings().clone();
        let new = Settings {
            hotkey: "CommandOrControl+Alt+C".into(),
            max_records: 1000,
            ..Settings::default()
        };
        let mut calls = Vec::new();
        let r = commit_settings(&state, new, |p, s, _| {
            calls.push((p.to_string(), s.to_string()));
            if p == "CommandOrControl+Alt+C" {
                Err(AppError::HotkeyTaken)
            } else {
                Ok(())
            }
        });
        assert!(matches!(r, Err(AppError::HotkeyTaken)));
        assert_eq!(*state.settings(), old);
        assert_eq!(state.store().load_settings(), old);
        assert_eq!(calls.first().unwrap().0, "CommandOrControl+Alt+C");
        assert_eq!(
            *calls.last().unwrap(),
            (old.hotkey.clone(), old.shot_hotkey.clone())
        );
    }

    #[test]
    fn shot_hotkey_change_restores_pair_on_failure() {
        let state = AppState::for_test();
        let old = state.settings().clone();
        let new = Settings {
            shot_hotkey: "Alt+Shift+S".into(),
            ..Settings::default()
        };
        let mut calls = Vec::new();
        let r = commit_settings(&state, new, |p, s, _| {
            calls.push((p.to_string(), s.to_string()));
            if s == "Alt+Shift+S" {
                Err(AppError::ShotHotkeyTaken)
            } else {
                Ok(())
            }
        });
        assert!(matches!(r, Err(AppError::ShotHotkeyTaken)));
        assert_eq!(*state.settings(), old);
        assert_eq!(state.store().load_settings(), old);
        assert_eq!(
            calls,
            [
                (old.hotkey.clone(), "Alt+Shift+S".to_string()),
                (old.hotkey.clone(), old.shot_hotkey.clone())
            ]
        );
    }

    #[test]
    fn rec_hotkey_change_restores_all_three_on_failure() {
        let state = AppState::for_test();
        let old = state.settings().clone();
        let new = Settings {
            rec_hotkey: "Alt+Shift+T".into(),
            ..Settings::default()
        };
        let mut calls = Vec::new();
        let r = commit_settings(&state, new, |p, s, r| {
            calls.push([p, s, r].map(String::from));
            if r == "Alt+Shift+T" {
                Err(AppError::RecHotkeyTaken)
            } else {
                Ok(())
            }
        });
        assert!(matches!(r, Err(AppError::RecHotkeyTaken)));
        assert_eq!(*state.settings(), old);
        assert_eq!(state.store().load_settings(), old);
        let (p, s) = (old.hotkey.clone(), old.shot_hotkey.clone());
        assert_eq!(
            calls,
            [
                [p.clone(), s.clone(), "Alt+Shift+T".into()],
                [p, s, old.rec_hotkey.clone()]
            ]
        );
    }

    #[test]
    fn unchanged_hotkey_is_not_reregistered() {
        let state = AppState::for_test();
        let new = Settings {
            max_records: 1000,
            ..Settings::default()
        };
        let mut calls = 0;
        let changed = commit_settings(&state, new, |_, _, _| {
            calls += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(calls, 0);
        assert_eq!(changed, ["maxRecords"]);
    }

    #[test]
    fn app_assembles_with_handlers() {
        tauri::test::mock_builder()
            .manage(AppState::for_test())
            .invoke_handler(tauri::generate_handler![
                // 带 AppHandle 参数的命令（含 get_record、finish_onboarding、set_dialog_open）只能用于 Wry 运行时，由 lib.rs 的 generate_handler! 编译覆盖
                bootstrap,
                prune_preview,
                crate::commands::panel::accessibility_granted,
                crate::commands::panel::screen_capture_granted,
                crate::commands::records::search,
                crate::commands::records::count_unpinned,
                crate::commands::records::ocr_progress,
                crate::commands::records::copy_text_plain,
            ])
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("app should build");
    }
}
