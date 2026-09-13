use tauri::State;

use crate::settings::{AppSettings, AppSettingsPatch};
use crate::AppState;

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, String> {
    Ok(state.settings.get())
}

#[tauri::command]
pub async fn update_settings(
    state: State<'_, AppState>,
    patch: AppSettingsPatch,
) -> Result<AppSettings, String> {
    state.settings.update(&patch)
}
