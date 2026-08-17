use crate::worksheet::{self, FileMetadata};

#[tauri::command]
pub fn get_file_metadata(path: &str) -> Result<FileMetadata, String> {
    worksheet::get_file_metadata(path)
}