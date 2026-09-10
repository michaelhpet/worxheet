use tauri::State;

use crate::settings::{AppSettings, AppSettingsPatch};
use crate::AppState;

/// The full in-memory settings document.
#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, String> {
    Ok(state.settings.get())
}

/// Merge a partial update into the persisted settings.
#[tauri::command]
pub async fn update_settings(
    state: State<'_, AppState>,
    patch: AppSettingsPatch,
) -> Result<AppSettings, String> {
    state.settings.update(&patch)
}