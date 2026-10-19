use std::sync::atomic::Ordering;

use serde::Serialize;
use tauri::{AppHandle, State};
use xpaste_core::{RecordSummary, Settings};

use crate::clock::now_ms;
use crate::error::AppError;
use crate::events::{PAUSED_CHANGED, emit, emit_removed};
use crate::state::AppState;

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
    })
}

#[tauri::command(async)]
pub fn bootstrap(state: State<'_, AppState>) -> Result<Bootstrap, AppError> {
    bootstrap_inner(&state)
}

/// 校验、持久化并生效新设置，然后按新的保留时长与条数上限清理；返回被删 id。不处理快捷键、不发事件。
pub fn save_settings_inner(state: &AppState, s: Settings) -> Result<Vec<i64>, AppError> {
    s.validate()?;
    state.store().save_settings(&s)?;
    let (days, max) = (s.retention_days, s.max_records);
    *state.settings() = s;
    let mut store = state.store();
    let mut removed = store.prune_retention(days, now_ms())?;
    removed.extend(store.prune_capacity(max)?);
    Ok(removed)
}

#[tauri::command(async)]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<(), AppError> {
    settings.validate()?;
    let old = state.settings().hotkey.clone();
    let hotkey_changed = settings.hotkey != old;
    // register 会先注销全部快捷键，失败时要把旧的注册回去
    let restore = |app: &AppHandle| {
        if let Err(e) = crate::hotkey::register(app, &old) {
            log::warn!("恢复原快捷键失败：{e}");
        }
    };
    if hotkey_changed && let Err(e) = crate::hotkey::register(&app, &settings.hotkey) {
        restore(&app);
        return Err(e);
    }
    match save_settings_inner(&state, settings) {
        Ok(removed) => {
            emit_removed(&app, &removed);
            Ok(())
        }
        Err(e) => {
            if hotkey_changed {
                restore(&app);
            }
            Err(e)
        }
    }
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

#[tauri::command(async)]
pub fn set_paused(app: AppHandle, state: State<'_, AppState>, on: bool) -> Result<(), AppError> {
    state.paused.store(on, Ordering::SeqCst);
    emit(&app, PAUSED_CHANGED, on);
    Ok(())
}

#[tauri::command(async)]
pub fn finish_onboarding(state: State<'_, AppState>) -> Result<(), AppError> {
    Ok(state.store().kv_set("onboarded", "1")?)
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
        assert_eq!(
            save_settings_inner(&state, s.clone()).unwrap(),
            Vec::<i64>::new()
        );
        assert_eq!(*state.settings(), s);
        assert_eq!(state.store().load_settings(), s);
    }

    #[test]
    fn app_assembles_with_handlers() {
        tauri::test::mock_builder()
            .manage(AppState::for_test())
            .invoke_handler(tauri::generate_handler![
                // 带 AppHandle 参数的命令只能用于 Wry 运行时，由 lib.rs 的 generate_handler! 编译覆盖
                bootstrap,
                prune_preview,
                finish_onboarding,
                crate::commands::records::get_record,
                crate::commands::records::search,
                crate::commands::records::count_unpinned,
            ])
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("app should build");
    }
}
