use serde::Serialize;
use std::string::String;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn temp_path(name: &str) -> String {
        let dir = std::env::temp_dir().join("worxheet-test");
        let _ = fs::create_dir_all(&dir);
        dir.join(name).to_string_lossy().to_string()
    }

    #[test]
    fn test_existing_file() {
        let path = temp_path("test_existing.txt");
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(b"hello").unwrap();
        let result = get_file_metadata(&path).unwrap();
        assert_eq!(result.name, "test_existing.txt");
        assert_eq!(result.extension, "txt");
        assert_eq!(result.size, 5);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_no_extension() {
        let path = temp_path("noext");
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(b"data").unwrap();
        let result = get_file_metadata(&path);
        assert!(result.is_err());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_missing_file() {
        let path = temp_path("does_not_exist.pdf");
        let result = get_file_metadata(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_dotted_extension() {
        let path = temp_path("archive.tar.gz");
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(b"data").unwrap();
        let result = get_file_metadata(&path).unwrap();
        assert_eq!(result.name, "archive.tar.gz");
        assert_eq!(result.extension, "gz");
        let _ = fs::remove_file(&path);
    }
}

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
