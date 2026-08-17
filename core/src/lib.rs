use std::sync::Arc;

use sqlx::{Pool, Sqlite};
use tauri::{Emitter, Manager};

use crate::models::{ModelFileKind, ModelPool, ProgressSink};
use crate::pipeline::{PipelineJobs, resume_stale};

mod commands;
mod database;
mod models;
mod pipeline;
mod schema;
mod worksheet;

pub struct AppState {
    pub(crate) database: Pool<Sqlite>,
    pub(crate) models: Arc<ModelPool>,
    pub(crate) jobs: Arc<PipelineJobs>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle();
            let pool = tauri::async_runtime::block_on(database::connect(handle))?;
            let app_data_dir = handle.path().app_data_dir().map_err(|e| e.to_string())?;
            let on_model_download: Arc<ProgressSink> = {
                let app = handle.clone();
                Arc::new(move |kind: ModelFileKind, done: u64, total: u64| {
                    let _ = app.emit(
                        "model-download",
                        serde_json::json!({
                            "kind": kind.as_str(),
                            "done": done,
                            "total": total,
                        }),
                    );
                })
            };
            let models = Arc::new(ModelPool::with_progress(
                models::models_dir(&app_data_dir),
                Some(on_model_download),
            ));
            let jobs = Arc::new(PipelineJobs::default());
            tauri::async_runtime::spawn(resume_stale(
                handle.clone(),
                pool.clone(),
                models.clone(),
                jobs.clone(),
            ));
            app.manage(AppState {
                database: pool,
                models,
                jobs,
            });
            Ok(())
        })
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::file::get_file_metadata,
            commands::worksheet::get_worksheets,
            commands::worksheet::get_worksheet,
            commands::worksheet::create_worksheet,
            commands::worksheet::delete_worksheet,
            commands::pipeline::get_artifacts,
            commands::pipeline::get_pipeline_status
        ])
        .run(tauri::generate_context!())
        .expect("Error while running application");
}