use std::sync::Arc;

use sqlx::{Pool, Sqlite};
use tauri::Manager;

use crate::models::ModelPool;

mod chunk;
mod database;
mod embed;
mod file;
mod generation;
mod ingest;
mod llm;
mod models;
mod pipeline;
mod retrieval;
mod schema;
mod worksheet;

pub struct AppState {
    pub(crate) database: Pool<Sqlite>,
    pub(crate) models: Arc<ModelPool>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle();
            let pool = tauri::async_runtime::block_on(async { database::connect(&handle).await })?;
            let app_data_dir = handle
                .path()
                .app_data_dir()
                .map_err(|e| e.to_string())?;
            let models = Arc::new(ModelPool::new(models::models_dir(&app_data_dir)));
            app.manage(AppState {
                database: pool,
                models,
            });
            return Ok(());
        })
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            file::get_file_metadata,
            worksheet::get_worksheets,
            worksheet::get_worksheet,
            worksheet::create_worksheet,
            worksheet::delete_worksheet,
            pipeline::get_files,
            pipeline::process_files,
            pipeline::embed_worksheet,
            pipeline::retrieve_chunks,
            pipeline::generate_artifacts,
            pipeline::get_artifacts
        ])
        .run(tauri::generate_context!())
        .expect("Error while running application");
}
