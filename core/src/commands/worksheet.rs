use tauri::State;

use crate::pipeline;
use crate::schema::Paginated;
use crate::worksheet::{self, Worksheet};
use crate::AppState;

#[tauri::command]
pub async fn get_worksheets(
    state: State<'_, AppState>,
    page: Option<i64>,
    per_page: Option<i64>,
) -> Result<Paginated<Worksheet>, String> {
    worksheet::get_worksheets(&state.database, page, per_page).await
}

#[tauri::command]
pub async fn get_worksheet(
    state: State<'_, AppState>,
    id: &str,
) -> Result<Worksheet, String> {
    worksheet::get_worksheet(&state.database, id).await
}

#[tauri::command]
pub async fn delete_worksheet(
    state: State<'_, AppState>,
    id: &str,
) -> Result<(), String> {
    pipeline::remove_job(&state.jobs, id);
    worksheet::delete_worksheet(&state.database, id).await
}

#[tauri::command]
pub async fn create_worksheet(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    name: &str,
    files: Vec<String>,
) -> Result<Worksheet, String> {
    let worksheet = worksheet::create_worksheet(&state.database, name, files.clone()).await?;
    if !files.is_empty() {
        pipeline::start_job(
            app,
            state.database.clone(),
            state.models.clone(),
            state.jobs.clone(),
            worksheet.id.clone(),
        );
    }
    Ok(worksheet)
}