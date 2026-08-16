use crate::file::{self, FileMetadata};

#[tauri::command]
pub fn get_file_metadata(path: &str) -> Result<FileMetadata, String> {
    file::get_file_metadata(path)
}