use std::sync::Arc;

use sqlx::{Pool, Sqlite};
use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{Emitter, Manager};

use crate::pipeline::{resume_stale, PipelineJobs};
use crate::settings::SettingsState;

mod commands;
mod database;
mod logging;
mod pipeline;
mod provider;
mod schema;
mod settings;
mod worksheet;

pub struct AppState {
    pub(crate) database: Pool<Sqlite>,
    pub(crate) settings: Arc<SettingsState>,
    pub(crate) jobs: Arc<PipelineJobs>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle();
            let pool = tauri::async_runtime::block_on(database::connect(handle))?;

            let settings_path = handle
                .path()
                .app_data_dir()
                .map_err(|e| format!("Failed to resolve app data dir: {e}"))?
                .join("settings.json");
            let settings = Arc::new(SettingsState::load(settings_path)?);
            let jobs = Arc::new(PipelineJobs::default());

            setup_native_menu(handle)?;
            tauri::async_runtime::spawn(resume_stale(
                handle.clone(),
                pool.clone(),
                settings.clone(),
                jobs.clone(),
            ));
            app.manage(AppState {
                database: pool,
                settings,
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
            commands::pipeline::retry_pipeline,
            commands::pipeline::regenerate_artifacts,
            commands::pipeline::stop_pipeline,
            commands::provider::get_provider_status,
            commands::provider::set_provider_config,
            commands::provider::list_provider_models,
            commands::settings::get_settings,
            commands::settings::update_settings
        ])
        .run(tauri::generate_context!())
        .expect("Error while running application");
}

/// Relays the native Preferences item as a `settings:open` event.
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
