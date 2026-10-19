use std::sync::atomic::Ordering;

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
pub fn save_settings_inner(state: &AppState, s: Settings) -> Result<(), AppError> {
    s.validate()?;
    state.store().save_settings(&s)?;
    *state.settings() = s;
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
    ]
    .into_iter()
    .filter_map(|(name, changed)| changed.then_some(name))
    .collect()
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

#[tauri::command(async)]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    mut settings: Settings,
) -> Result<(), AppError> {
    settings.validate()?;
    let old = state.settings().clone();
    // 面板尺寸只由拖动记忆（panel::remember_size）写；前端手里的快照可能是旧的，不让它覆盖
    settings.panel_size = old.panel_size;
    let changed = changed_fields(&old, &settings);
    let hotkey_changed = changed.contains(&"hotkey");
    let (hotkey, appearance, launch) = (
        settings.hotkey.clone(),
        settings.appearance,
        settings.launch_at_login,
    );
    // register 会先注销全部快捷键，失败时要把旧的注册回去
    let restore = |app: &AppHandle| {
        if let Err(e) = crate::hotkey::register(app, &old.hotkey) {
            log::warn!("恢复原快捷键失败：{e}");
        }
    };
    if hotkey_changed && let Err(e) = crate::hotkey::register(&app, &hotkey) {
        restore(&app);
        return Err(e);
    }
    if let Err(e) = save_settings_inner(&state, settings) {
        if hotkey_changed {
            restore(&app);
        }
        return Err(e);
    }
    log::info!("设置已保存 changed=[{}]", changed.join(", "));
    // 设置已持久化并生效：以下步骤失败不回滚、不算保存失败（都只记日志）
    let removed = prune_to_settings(&state);
    emit_removed(&app, &removed);
    if changed.contains(&"appearance") {
        crate::panel::apply_theme(&app, appearance);
    }
    if hotkey_changed {
        crate::tray::set_hotkey_label(&hotkey);
    }
    if changed.contains(&"launchAtLogin") {
        set_autostart(&app, launch);
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

/// 命令与托盘菜单共用：写暂停标志、更新托盘、通知前端。
pub fn set_paused_inner(app: &AppHandle, on: bool) {
    app.state::<AppState>().paused.store(on, Ordering::SeqCst);
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
    state.store().kv_set("onboarded", "1")?;
    state.onboarding.store(false, Ordering::SeqCst);
    let launch = state.settings().launch_at_login;
    if launch {
        set_autostart(&app, true);
    }
    Ok(())
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
    fn changed_fields_lists_only_differences() {
        let old = Settings::default();
        assert!(changed_fields(&old, &old).is_empty());
        let new = Settings {
            hotkey: "Alt+Space".into(),
            appearance: xpaste_core::Appearance::Dark,
            panel_size: (1000.0, 700.0),
            ..Settings::default()
        };
        assert_eq!(
            changed_fields(&old, &new),
            ["hotkey", "appearance", "panelSize"]
        );
    }

    #[test]
    fn app_assembles_with_handlers() {
        tauri::test::mock_builder()
            .manage(AppState::for_test())
            .invoke_handler(tauri::generate_handler![
                // 带 AppHandle 参数的命令（含 get_record、finish_onboarding）只能用于 Wry 运行时，由 lib.rs 的 generate_handler! 编译覆盖
                bootstrap,
                prune_preview,
                crate::commands::panel::accessibility_granted,
                crate::commands::records::search,
                crate::commands::records::count_unpinned,
            ])
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("app should build");
    }
}
