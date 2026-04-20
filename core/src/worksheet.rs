use serde::{Deserialize, Serialize};
use tauri::State;
use time::OffsetDateTime;
use ulid::Ulid;

use crate::AppState;

#[derive(Serialize, Deserialize)]
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

    let query = sqlx::query_as!(Worksheet, "SELECT * FROM worksheets");
    let worksheets = match query.fetch_all(pool).await {
        Err(_) => return Err(String::from("Could not fetch worksheets")),
        Ok(worksheets) => worksheets,
    };

    Ok(worksheets)
}

#[tauri::command]
pub async fn create_worksheet(state: State<'_, AppState>, name: &str) -> Result<Worksheet, String> {
    let pool = &state.database;

    let worksheet = Worksheet::new(name);
    let query = sqlx::query!(
        "INSERT INTO worksheets (id, name) VALUES (?, ?)",
        worksheet.id,
        worksheet.name
    );
    match query.execute(pool).await {
        Err(_) => return Err(String::from("Failed to create new worksheet")),
        _ => (),
    };

    Ok(worksheet)
}
