use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqliteSynchronous},
    Pool, Sqlite,
};
use std::str::FromStr;
use tauri::{AppHandle, Manager};

async fn create_pool(url: &str) -> Result<Pool<Sqlite>, sqlx::Error> {
    println!("Creating database pool...");
    let options = SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal);
    let pool = SqlitePool::connect_with(options).await?;
    println!("Database pool created successfully");
    Ok(pool)
}

async fn run_migrations(pool: &Pool<Sqlite>) -> Result<(), sqlx::Error> {
    println!("Running database migrations...");
    sqlx::migrate!("./migrations").run(pool).await?;
    println!("Database migrations completed successfully");
    Ok(())
}

pub async fn connect(app: &AppHandle) -> Result<Pool<Sqlite>, String> {
    let app_dir = app
        .path()
        .app_data_dir()
        .expect("Could not read app data directory");

    std::fs::create_dir_all(&app_dir).expect("Could not read app data directory");

    let db_path = app_dir.join("database.sqlite");
    let db_url = format!("sqlite:{}", db_path.display());
    println!("Connecting to database: {}...", &db_url);

    let pool = create_pool(&db_url)
        .await
        .expect("Could not connect to database");

    run_migrations(&pool)
        .await
        .expect("Failed to run database migrations");

    println!("Database connected with {} size pool...", &pool.size());

    Ok(pool)
}
