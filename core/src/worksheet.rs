use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use time::OffsetDateTime;
use ulid::Ulid;

use crate::schema::Paginated;

#[derive(Serialize, Deserialize)]
pub struct Worksheet {
    pub id: String,
    pub name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// One of `idle`/`running`/`done`/`failed`.
    pub pipeline_status: String,
    pub file_count: i64,
    /// Distinct, uppercased file extensions, e.g. `["PDF", "DOCX"]`.
    pub file_extensions: Vec<String>,
    /// Counts for the quiz artifact types only.
    pub quiz_counts: HashMap<String, i64>,
}

#[derive(Serialize)]
pub struct WorksheetDetail {
    pub id: String,
    pub name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    pub pipeline_status: String,
    pub pipeline_error: Option<String>,
    pub file_count: i64,
    pub file_extensions: Vec<String>,
    pub artifact_counts: HashMap<String, i64>,
}

const QUIZ_ARTIFACT_TYPES: [&str; 3] = ["MultipleChoiceQuiz", "EssayQuiz", "CompletionQuiz"];

impl Worksheet {
    pub fn new(name: &str) -> Self {
        Self {
            id: Ulid::new().to_string(),
            name: String::from(name),
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
            pipeline_status: String::from("idle"),
            file_count: 0,
            file_extensions: Vec::new(),
            quiz_counts: HashMap::new(),
        }
    }
}

/// For each worksheet id, its file count and distinct uppercased extensions.
async fn aggregate_file_extensions(
    pool: &sqlx::SqlitePool,
    ids: &[(String,)],
) -> Result<HashMap<String, (i64, Vec<String>)>, String> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT worksheet_id, extension FROM files")
        .fetch_all(pool)
        .await
        .map_err(|_| String::from("Failed to query file extensions"))?;

    let id_set: std::collections::HashSet<String> = ids.iter().map(|(id,)| id.clone()).collect();
    let mut out: HashMap<String, (i64, Vec<String>)> = HashMap::new();
    for (worksheet_id, extension) in rows {
        if !id_set.contains(&worksheet_id) {
            continue;
        }
        let entry = out.entry(worksheet_id).or_insert((0, Vec::new()));
        entry.0 += 1;
        let ext = extension.to_uppercase();
        if !entry.1.contains(&ext) {
            entry.1.push(ext);
        }
    }
    Ok(out)
}

pub async fn get_worksheets(
    pool: &sqlx::SqlitePool,
    page: Option<i64>,
    per_page: Option<i64>,
) -> Result<Paginated<Worksheet>, String> {
    let page = page.unwrap_or(1).max(1);
    let per_page = per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM worksheets")
        .fetch_one(pool)
        .await
        .map_err(|_| String::from("Failed to count worksheets"))?;

    let base: Vec<(String, String, OffsetDateTime, OffsetDateTime, String)> = sqlx::query_as(
        "SELECT id, name, created_at, updated_at, pipeline_status
         FROM worksheets
         ORDER BY created_at DESC LIMIT ? OFFSET ?",
    )
    .bind(per_page)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(|_| String::from("Could not fetch worksheets"))?;

    let total_pages = (total.0 as f64 / per_page as f64).ceil() as i64;

    let page_ids: Vec<(String,)> = base.iter().map(|(id, ..)| (id.clone(),)).collect();
    let files = aggregate_file_extensions(pool, &page_ids).await?;
    let quiz_counts = aggregate_quiz_counts(pool, &page_ids).await?;

    let items = base
        .into_iter()
        .map(|(id, name, created_at, updated_at, pipeline_status)| {
            let (file_count, extensions) = files.get(&id).cloned().unwrap_or((0, Vec::new()));
            let quiz = quiz_counts.get(&id).cloned().unwrap_or_default();
            Worksheet {
                id,
                name,
                created_at,
                updated_at,
                pipeline_status,
                file_count,
                file_extensions: extensions,
                quiz_counts: quiz,
            }
        })
        .collect();

    Ok(Paginated {
        items,
        total: total.0,
        page,
        per_page,
        total_pages,
    })
}

/// Quiz-artifact counts keyed by worksheet id.
async fn aggregate_quiz_counts(
    pool: &sqlx::SqlitePool,
    ids: &[(String,)],
) -> Result<HashMap<String, HashMap<String, i64>>, String> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let id_set: std::collections::HashSet<String> = ids.iter().map(|(id,)| id.clone()).collect();

    let mut placeholders = Vec::new();
    for i in 0..QUIZ_ARTIFACT_TYPES.len() {
        placeholders.push(format!("?{}", i + 1));
    }
    let in_list = placeholders.join(", ");

    let sql = format!(
        "SELECT worksheet_id, artifact_type, COUNT(*)
         FROM artifacts
         WHERE artifact_type IN ({in_list})
         GROUP BY worksheet_id, artifact_type"
    );

    let mut query = sqlx::query_as::<_, (String, String, i64)>(&sql);
    for art_type in QUIZ_ARTIFACT_TYPES {
        query = query.bind(art_type);
    }
    let rows = query
        .fetch_all(pool)
        .await
        .map_err(|_| String::from("Failed to query quiz counts"))?;

    let mut out: HashMap<String, HashMap<String, i64>> = HashMap::new();
    for (worksheet_id, artifact_type, count) in rows {
        if !id_set.contains(&worksheet_id) {
            continue;
        }
        out.entry(worksheet_id)
            .or_default()
            .insert(artifact_type, count);
    }
    Ok(out)
}

pub async fn get_worksheet(pool: &sqlx::SqlitePool, id: &str) -> Result<WorksheetDetail, String> {
    let (name, created_at, updated_at, pipeline_status, pipeline_error): (
        String,
        OffsetDateTime,
        OffsetDateTime,
        String,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT name, created_at, updated_at, pipeline_status, pipeline_error
         FROM worksheets WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|_| String::from("Failed to fetch worksheet"))?
    .ok_or_else(|| String::from("Worksheet not found"))?;

    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT artifact_type, COUNT(*) FROM artifacts WHERE worksheet_id = ? GROUP BY artifact_type")
            .bind(id)
            .fetch_all(pool)
            .await
            .map_err(|_| String::from("Failed to fetch artifact counts"))?;

    let artifact_counts = rows.into_iter().collect();

    let (file_count, file_extensions) = aggregate_file_extensions(pool, &[(id.to_string(),)])
        .await?
        .remove(id)
        .unwrap_or((0, Vec::new()));

    Ok(WorksheetDetail {
        id: id.to_string(),
        name,
        created_at,
        updated_at,
        pipeline_status,
        pipeline_error,
        file_count,
        file_extensions,
        artifact_counts,
    })
}

pub async fn delete_worksheet(pool: &sqlx::SqlitePool, id: &str) -> Result<(), String> {
    sqlx::query("DELETE FROM worksheets WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|_| String::from("Failed to delete worksheet"))?;

    Ok(())
}

pub async fn create_worksheet(
    pool: &sqlx::SqlitePool,
    name: &str,
    files: Vec<String>,
) -> Result<Worksheet, String> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| String::from("Failed to begin transaction"))?;

    let worksheet = Worksheet::new(name);
    sqlx::query("INSERT INTO worksheets (id, name) VALUES (?, ?)")
        .bind(&worksheet.id)
        .bind(&worksheet.name)
        .execute(&mut *transaction)
        .await
        .map_err(|_| String::from("Failed to create new worksheet"))?;

    for file_path in &files {
        let path = std::path::Path::new(file_path);
        let file_name = path
            .file_name()
            .ok_or_else(|| format!("Invalid file path: {}", file_path))?
            .to_string_lossy()
            .to_string();
        let extension = path
            .extension()
            .ok_or_else(|| format!("File has no extension: {}", file_path))?
            .to_string_lossy()
            .to_string();
        let size = std::fs::metadata(file_path)
            .map_err(|e| format!("Failed to read file '{}': {}", file_path, e))?
            .len();
        let sha256 = sha256_of_path(file_path)?;

        let file_id = Ulid::new().to_string();

        sqlx::query(
            "INSERT INTO files (id, worksheet_id, path, name, extension, size, sha256) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&file_id)
        .bind(&worksheet.id)
        .bind(file_path)
        .bind(&file_name)
        .bind(&extension)
        .bind(size as i64)
        .bind(&sha256)
        .execute(&mut *transaction)
        .await
        .map_err(|_| String::from("Failed to register file"))?;
    }

    transaction
        .commit()
        .await
        .map_err(|_| String::from("Failed to commit transaction"))?;

    Ok(worksheet)
}

#[derive(Serialize)]
pub struct FileMetadata {
    name: String,
    extension: String,
    size: u64,
}

/// Content fingerprint of a file, used to detect an already-ingested copy of
/// the same source so its chunks can be reused without re-parsing.
pub fn sha256_of_path(path: &str) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).map_err(|e| format!("Failed to read '{}': {}", path, e))?;
    let digest = Sha256::digest(&bytes);
    Ok(hex::encode(digest))
}

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

    Ok(FileMetadata {
        name,
        extension,
        size: metadata.len(),
    })
}

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
