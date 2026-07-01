use sqlx::{Pool, Sqlite};
use tauri::Manager;

mod chunk;
mod database;
mod file;
mod ingest;
mod schema;
mod worksheet;

pub struct AppState {
    pub(crate) database: Pool<Sqlite>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle();
            let pool = tauri::async_runtime::block_on(async { database::connect(&handle).await })?;
            app.manage(AppState { database: pool });
            return Ok(());
        })
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            file::get_file_metadata,
            worksheet::get_worksheets,
            worksheet::get_worksheet,
            worksheet::create_worksheet
        ])
        .run(tauri::generate_context!())
        .expect("Error while running application");
}
