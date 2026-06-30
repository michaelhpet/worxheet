use serde::Serialize;
use std::string::String;

#[derive(Serialize)]
pub struct FileMetadata {
    name: String,
    extension: String,
    size: u64,
}

#[tauri::command]
pub fn get_file_metadata(path: &str) -> Result<FileMetadata, String> {
    let path_data = std::path::Path::new(path);

    let name = match path_data.file_name() {
        None => return Err(String::from("Failed to get file name")),
        Some(name) => name.to_string_lossy().to_string(),
    };

    let extension = match path_data.extension() {
        None => return Err(String::from("Failed to get file extension")),
        Some(extension) => extension.to_string_lossy().to_string(),
    };

    let metadata = match path_data.metadata() {
        Err(error) => return Err(error.to_string()),
        Ok(metadata) => metadata,
    };

    return Ok(FileMetadata {
        name,
        extension,
        size: metadata.len(),
    });
}
