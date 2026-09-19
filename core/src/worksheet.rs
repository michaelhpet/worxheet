use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use time::OffsetDateTime;
use ulid::Ulid;

use crate::schema::{ArtifactType, Paginated};

#[derive(Serialize, Deserialize)]
pub struct Worksheet {
    pub id: String,
    pub name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    pub pipeline_status: String,
    pub file_count: i64,
    pub file_extensions: Vec<String>,
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

async fn aggregate_file_extensions(
    pool: &sqlx::SqlitePool,
    ids: &[String],
) -> Result<HashMap<String, (i64, Vec<String>)>, String> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut builder = sqlx::QueryBuilder::new(
        "SELECT worksheet_id, COUNT(*), GROUP_CONCAT(DISTINCT UPPER(extension))
         FROM files WHERE worksheet_id IN (",
    );
    let mut separated = builder.separated(", ");
    for id in ids {
        separated.push_bind(id);
    }
    separated.push_unseparated(") GROUP BY worksheet_id");

    let rows: Vec<(String, i64, Option<String>)> =
        builder
            .build_query_as()
            .fetch_all(pool)
            .await
            .map_err(|_| String::from("Failed to query file extensions"))?;

    Ok(rows
        .into_iter()
        .map(|(worksheet_id, count, extensions)| {
            let mut extensions: Vec<String> = extensions
                .map(|list| list.split(',').map(String::from).collect())
                .unwrap_or_default();
            extensions.sort();
            (worksheet_id, (count, extensions))
        })
        .collect())
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

    let total_pages = (total.0 + per_page - 1) / per_page;

    let page_ids: Vec<String> = base.iter().map(|(id, ..)| id.clone()).collect();
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

async fn aggregate_quiz_counts(
    pool: &sqlx::SqlitePool,
    ids: &[String],
) -> Result<HashMap<String, HashMap<String, i64>>, String> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut builder = sqlx::QueryBuilder::new(
        "SELECT worksheet_id, artifact_type, COUNT(*)
         FROM artifacts WHERE artifact_type IN (",
    );
    let mut separated = builder.separated(", ");
    for quiz_type in ArtifactType::QUIZ {
        separated.push_bind(quiz_type.to_db());
    }
    separated.push_unseparated(") AND worksheet_id IN (");
    let mut separated = builder.separated(", ");
    for id in ids {
        separated.push_bind(id);
    }
    separated.push_unseparated(") GROUP BY worksheet_id, artifact_type");

    let rows: Vec<(String, String, i64)> = builder
        .build_query_as()
        .fetch_all(pool)
        .await
        .map_err(|_| String::from("Failed to query quiz counts"))?;

    let mut out: HashMap<String, HashMap<String, i64>> = HashMap::new();
    for (worksheet_id, artifact_type, count) in rows {
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

    let (file_count, file_extensions) =
        aggregate_file_extensions(pool, std::slice::from_ref(&id.to_string()))
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
    let result = sqlx::query("DELETE FROM worksheets WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|_| String::from("Failed to delete worksheet"))?;
    if result.rows_affected() == 0 {
        return Err(String::from("Worksheet not found"));
    }

    Ok(())
}

/// Extensions the ingest pipeline can actually parse. Must stay in sync with
/// `parse_blocks` in pipeline/ingest.rs and `SUPPORTED_EXTENSIONS` in app/.
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "pdf", "jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp", "svg", "pptx",
    "docx", "ppt", "doc", "txt", "md", "csv",
];

fn ensure_supported_extension(extension: &str, path: &str) -> Result<(), String> {
    if SUPPORTED_EXTENSIONS.contains(&extension.to_lowercase().as_str()) {
        Ok(())
    } else {
        Err(format!("Unsupported file extension '{extension}' for '{path}'"))
    }
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
        let parts = file_parts(file_path)?;
        ensure_supported_extension(&parts.extension, file_path)?;
        let identity = identity_key(file_path, &parts);

        let file_id = Ulid::new().to_string();

        sqlx::query(
            "INSERT INTO files (id, worksheet_id, path, name, extension, size, identity_key) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&file_id)
        .bind(&worksheet.id)
        .bind(file_path)
        .bind(&parts.name)
        .bind(&parts.extension)
        .bind(parts.size as i64)
        .bind(&identity)
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

struct FileParts {
    name: String,
    extension: String,
    size: u64,
    modified_nanos: u128,
}

fn file_parts(path: &str) -> Result<FileParts, String> {
    let parsed = std::path::Path::new(path);
    let name = parsed
        .file_name()
        .ok_or_else(|| format!("Invalid file path: {path}"))?
        .to_string_lossy()
        .to_string();
    let extension = parsed
        .extension()
        .ok_or_else(|| format!("File has no extension: {path}"))?
        .to_string_lossy()
        .to_string();
    let metadata = std::fs::metadata(path)
        .map_err(|e| format!("Failed to read metadata for '{path}': {e}"))?;
    let modified_nanos = metadata
        .modified()
        .map_err(|e| format!("Failed to read modified time for '{path}': {e}"))?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("Failed to read modified time for '{path}': {e}"))?
        .as_nanos();
    Ok(FileParts {
        name,
        extension,
        size: metadata.len(),
        modified_nanos,
    })
}

/// Absolute path + size + mtime; an edited file bumps mtime, forcing a re-parse.
fn identity_key(path: &str, parts: &FileParts) -> String {
    format!("{path}\u{1f}{}\u{1f}{}", parts.size, parts.modified_nanos)
}

pub fn get_file_metadata(path: &str) -> Result<FileMetadata, String> {
    let parts = file_parts(path)?;
    Ok(FileMetadata {
        name: parts.name,
        extension: parts.extension,
        size: parts.size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    async fn setup_db() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("in-memory pool");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("migrations");
        pool
    }

    async fn seed_worksheet(pool: &sqlx::SqlitePool, id: &str) {
        sqlx::query("INSERT INTO worksheets (id, name) VALUES (?, ?)")
            .bind(id)
            .bind("test")
            .execute(pool)
            .await
            .unwrap();
    }

    async fn seed_file(pool: &sqlx::SqlitePool, worksheet_id: &str, extension: &str) {
        sqlx::query(
            "INSERT INTO files (id, worksheet_id, path, name, extension, size, identity_key)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(ulid::Ulid::new().to_string())
        .bind(worksheet_id)
        .bind("/tmp/x")
        .bind("x")
        .bind(extension)
        .bind(1i64)
        .bind("k")
        .execute(pool)
        .await
        .unwrap();
    }

    async fn seed_artifact(pool: &sqlx::SqlitePool, worksheet_id: &str, artifact_type: &str) {
        sqlx::query(
            "INSERT INTO artifacts (id, worksheet_id, artifact_type, source, content)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(ulid::Ulid::new().to_string())
        .bind(worksheet_id)
        .bind(artifact_type)
        .bind("src")
        .bind("{}")
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_aggregates_scope_to_requested_worksheets() {
        let pool = setup_db().await;
        seed_worksheet(&pool, "ws-1").await;
        seed_worksheet(&pool, "ws-2").await;
        seed_file(&pool, "ws-1", "pdf").await;
        seed_file(&pool, "ws-1", "PDF").await;
        seed_file(&pool, "ws-1", "docx").await;
        seed_file(&pool, "ws-2", "png").await;
        seed_artifact(&pool, "ws-1", "MultipleChoiceQuiz").await;
        seed_artifact(&pool, "ws-1", "MultipleChoiceQuiz").await;
        seed_artifact(&pool, "ws-2", "EssayQuiz").await;

        let files = aggregate_file_extensions(&pool, &["ws-1".to_string()])
            .await
            .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            files["ws-1"],
            (3, vec!["DOCX".to_string(), "PDF".to_string()])
        );

        let counts = aggregate_quiz_counts(&pool, &["ws-1".to_string()])
            .await
            .unwrap();
        assert_eq!(counts.len(), 1);
        assert_eq!(counts["ws-1"]["MultipleChoiceQuiz"], 2);

        assert!(aggregate_file_extensions(&pool, &[])
            .await
            .unwrap()
            .is_empty());
    }

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

    #[test]
    fn test_supported_extensions_cover_text_and_images() {
        for ext in ["pdf", "png", "txt", "md", "csv", "docx", "pptx"] {
            assert!(SUPPORTED_EXTENSIONS.contains(&ext));
        }
        assert!(ensure_supported_extension("txt", "/tmp/a.txt").is_ok());
        assert!(ensure_supported_extension("MD", "/tmp/a.md").is_ok());
        assert!(ensure_supported_extension("mp3", "/tmp/a.mp3").is_err());
        assert!(ensure_supported_extension("heic", "/tmp/a.heic").is_err());
    }

    fn identity_of(path: &str) -> (String, u64) {
        let parts = file_parts(path).unwrap();
        (identity_key(path, &parts), parts.size)
    }

    #[test]
    fn test_file_identity_key_stable_for_unchanged_file() {
        let path = temp_path("identity_a.txt");
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(b"hello").unwrap();

        let (key1, size1) = identity_of(&path);
        let (key2, size2) = identity_of(&path);
        assert_eq!(size1, 5);
        assert_eq!(size2, 5);
        assert_eq!(key1, key2, "unchanged file must keep its identity key");
        assert!(key1.contains(&path));

        fs::remove_file(&path).unwrap();

        let missing = file_parts(&path);
        assert!(missing.is_err(), "missing file must error");
    }

    #[test]
    fn test_file_identity_key_changes_when_file_rewritten() {
        let path = temp_path("identity_b.txt");
        fs::write(&path, b"hello").unwrap();
        let (before, _) = identity_of(&path);

        fs::write(&path, b"hello world!").unwrap();
        let (after, size_after) = identity_of(&path);

        assert_ne!(
            before, after,
            "rewriting the file must change its identity key"
        );
        assert_eq!(size_after, 12);
        fs::remove_file(&path).unwrap();
    }
}
