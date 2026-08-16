use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;
use time::OffsetDateTime;
use ulid::Ulid;

use crate::schema::Paginated;

#[derive(Serialize, Deserialize, FromRow)]
pub struct Worksheet {
    pub id: String,
    pub name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl Worksheet {
    pub fn new(name: &str) -> Self {
        Self {
            id: Ulid::new().to_string(),
            name: String::from(name),
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        }
    }
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

    let items = sqlx::query_as::<_, Worksheet>(
        "SELECT * FROM worksheets ORDER BY created_at DESC LIMIT ? OFFSET ?",
    )
    .bind(per_page)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(|_| String::from("Could not fetch worksheets"))?;

    let total_pages = (total.0 as f64 / per_page as f64).ceil() as i64;

    Ok(Paginated {
        items,
        total: total.0,
        page,
        per_page,
        total_pages,
    })
}

pub async fn get_worksheet(pool: &sqlx::SqlitePool, id: &str) -> Result<Worksheet, String> {
    sqlx::query_as::<_, Worksheet>("SELECT * FROM worksheets WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|_| String::from("Failed to fetch worksheet"))?
        .ok_or_else(|| String::from("Worksheet not found"))
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

        let file_id = Ulid::new().to_string();

        sqlx::query(
            "INSERT INTO files (id, worksheet_id, path, name, extension, size) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&file_id)
        .bind(&worksheet.id)
        .bind(file_path)
        .bind(&file_name)
        .bind(&extension)
        .bind(size as i64)
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
