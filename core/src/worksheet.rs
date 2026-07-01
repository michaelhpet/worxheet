use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;
use tauri::State;
use time::OffsetDateTime;
use ulid::Ulid;

use crate::AppState;

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

#[tauri::command]
pub async fn get_worksheets(state: State<'_, AppState>) -> Result<Vec<Worksheet>, String> {
    let pool = &state.database;

    let query = sqlx::query_as::<_, Worksheet>("SELECT * FROM worksheets");
    let worksheets = match query.fetch_all(pool).await {
        Err(_) => return Err(String::from("Could not fetch worksheets")),
        Ok(worksheets) => worksheets,
    };

    Ok(worksheets)
}

#[tauri::command]
pub async fn create_worksheet(
    state: State<'_, AppState>,
    name: &str,
    files: Vec<String>,
) -> Result<Worksheet, String> {
    let pool = &state.database;

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
