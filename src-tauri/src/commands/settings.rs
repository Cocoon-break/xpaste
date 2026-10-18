use serde::Serialize;
use tauri::State;
use xpaste_core::Settings;

use crate::error::AppError;
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    pub os: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    pub settings: Settings,
    pub platform: PlatformInfo,
}

pub fn bootstrap_inner(state: &AppState) -> Bootstrap {
    Bootstrap {
        settings: state.settings.lock().unwrap().clone(),
        platform: PlatformInfo {
            os: xpaste_platform::OS,
        },
    }
}

#[tauri::command(async)]
pub fn bootstrap(state: State<'_, AppState>) -> Result<Bootstrap, AppError> {
    Ok(bootstrap_inner(&state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_returns_defaults_and_platform() {
        let state = AppState::new(Settings::default());
        let b = bootstrap_inner(&state);
        assert_eq!(b.settings.max_records, 500);
        assert_eq!(b.platform.os, xpaste_platform::OS);
    }

    #[test]
    fn app_assembles_with_handlers() {
        tauri::test::mock_builder()
            .manage(AppState::new(Settings::default()))
            .invoke_handler(tauri::generate_handler![bootstrap])
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("app should build");
    }
}
