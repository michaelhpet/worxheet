use std::path::PathBuf;
use std::sync::Arc;

use sqlx::{Pool, Sqlite};
use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{Emitter, Manager};

use crate::embedder::{EmbedderPool, DownloadProgress};
use crate::provider::config::{ProviderConfig, ProviderState};
use crate::pipeline::{PipelineJobs, resume_stale};

mod commands;
mod database;
mod embedder;
mod pipeline;
mod provider;
mod schema;
mod worksheet;

pub struct AppState {
    pub(crate) database: Pool<Sqlite>,
    pub(crate) embedder: Arc<EmbedderPool>,
    pub(crate) models_dir: PathBuf,
    pub(crate) providers: Arc<ProviderState>,
    pub(crate) jobs: Arc<PipelineJobs>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle();
            let pool = tauri::async_runtime::block_on(database::connect(handle))?;
            let app_data_dir = handle.path().app_data_dir().map_err(|e| e.to_string())?;
            let models_dir = embedder::models_dir(&app_data_dir);

            // Surface embedding-model download progress on the existing event.
            let on_model_download: DownloadProgress = {
                let app = handle.clone();
                Arc::new(move |done: u64, total: u64| {
                    let _ = app.emit(
                        "model-download",
                        serde_json::json!({
                            "kind": "embedding",
                            "done": done,
                            "total": total,
                        }),
                    );
                })
            };

            let embedder = Arc::new(EmbedderPool::new(Some(on_model_download)));
            let providers = Arc::new(ProviderState::new(ProviderConfig::default()));
            let jobs = Arc::new(PipelineJobs::default());

            setup_native_menu(handle)?;
            tauri::async_runtime::spawn(resume_stale(
                handle.clone(),
                pool.clone(),
                embedder.clone(),
                models_dir.clone(),
                providers.clone(),
                jobs.clone(),
            ));
            app.manage(AppState {
                database: pool,
                embedder,
                models_dir,
                providers,
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
            commands::pipeline::get_pipeline_status,
            commands::provider::get_provider_status,
            commands::provider::set_provider_config,
            commands::provider::validate_provider,
            commands::provider::list_provider_models
        ])
        .run(tauri::generate_context!())
        .expect("Error while running application");
}

/// Native application menu. The macOS app-name submenu carries the
/// conventional "Preferences…" item (⌘,); selecting it relays a
/// `settings:open` event the frontend settings dialog listens for.
fn setup_native_menu(handle: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let preferences = MenuItemBuilder::with_id("preferences", "Preferences…")
        .accelerator("CmdOrCtrl+Comma")
        .build(handle)?;

    let app_submenu = SubmenuBuilder::new(handle, "Worxheet")
        .about(None)
        .separator()
        .item(&preferences)
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let edit_submenu = SubmenuBuilder::new(handle, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    let window_submenu = SubmenuBuilder::new(handle, "Window")
        .minimize()
        .close_window()
        .build()?;

    let menu = MenuBuilder::new(handle)
        .item(&app_submenu)
        .item(&edit_submenu)
        .item(&window_submenu)
        .build()?;

    handle.set_menu(menu)?;

    handle.on_menu_event(|app, event| {
        if event.id() == "preferences" {
            let _ = app.emit("settings:open", ());
        }
    });

    Ok(())
}
