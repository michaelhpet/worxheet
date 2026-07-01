use tokenizers::Tokenizer;

use crate::chunk::chunk_text;
use crate::schema::Chunk;

const CHUNK_SIZE: usize = 512;
const CHUNK_OVERLAP: usize = 128;

pub fn parse_file(path: &str, extension: &str) -> Result<String, String> {
    match extension.to_lowercase().as_str() {
        "pdf" => {
            let doc = pdf_oxide::PdfDocument::open(path)
                .map_err(|e| format!("Failed to open PDF: {}", e))?;
            let page_count = doc
                .page_count()
                .map_err(|e| format!("Failed to get page count: {}", e))?;
            let mut text = String::new();
            for i in 0..page_count {
                let page_text = doc
                    .extract_text_auto(i)
                    .map_err(|e| format!("Failed to extract page {}: {}", i, e))?;
                text.push_str(&page_text);
                text.push('\n');
            }
            Ok(text)
        }
        "pptx" | "docx" | "ppt" | "doc" => {
            office_oxide::extract_text(path).map_err(|e| format!("Failed to extract text: {}", e))
        }
        _ => Err(format!("Unsupported file extension: {}", extension)),
    }
}

pub async fn process_files(
    pool: &sqlx::SqlitePool,
    worksheet_id: &str,
    file_ids: &[String],
    tokenizer: &Tokenizer,
) -> Result<Vec<Chunk>, String> {
    let mut all_chunks = Vec::new();
    let mut position_counter = 0i32;

    for file_id in file_ids {
        let row = sqlx::query_as::<_, (String, String, String)>(
            "SELECT path, extension, name FROM files WHERE id = ? AND worksheet_id = ?",
        )
        .bind(file_id)
        .bind(worksheet_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| format!("Failed to query file {}", file_id))?
        .ok_or_else(|| format!("File not found: {}", file_id))?;

        let (path, extension, _name) = row;

        sqlx::query("UPDATE files SET status = 'parsing' WHERE id = ?")
            .bind(file_id)
            .execute(pool)
            .await
            .map_err(|_| format!("Failed to update file status"))?;

        let text = parse_file(&path, &extension)?;

        let chunks = chunk_text(&text, CHUNK_SIZE, CHUNK_OVERLAP, tokenizer)?;

        for chunk_text in &chunks {
            let chunk_id = ulid::Ulid::new().to_string();
            sqlx::query(
                "INSERT INTO chunks (id, worksheet_id, file_id, position, text) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&chunk_id)
            .bind(worksheet_id)
            .bind(file_id)
            .bind(position_counter)
            .bind(chunk_text)
            .execute(pool)
            .await
            .map_err(|_| String::from("Failed to insert chunk"))?;

            all_chunks.push(Chunk {
                id: chunk_id,
                worksheet_id: worksheet_id.to_string(),
                file_id: file_id.to_string(),
                position: position_counter,
                text: chunk_text.clone(),
                embedding: None,
            });

            position_counter += 1;
        }

        sqlx::query("UPDATE files SET status = 'parsed' WHERE id = ?")
            .bind(file_id)
            .execute(pool)
            .await
            .map_err(|_| format!("Failed to update file status"))?;
    }

    sqlx::query("UPDATE worksheets SET updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(worksheet_id)
        .execute(pool)
        .await
        .map_err(|_| String::from("Failed to update worksheet"))?;

    Ok(all_chunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;
    use tokenizers::models::bpe::BPE;
    use tokenizers::Tokenizer;

    const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

    fn test_tokenizer() -> Tokenizer {
        let bpe = BPE::builder()
            .vocab_and_merges([("<unk>".to_string(), 0u32)], vec![])
            .unk_token("<unk>".to_string())
            .build()
            .unwrap();
        Tokenizer::new(bpe)
    }

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("Failed to create in-memory pool");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("Failed to run migrations");
        pool
    }

    async fn seed_worksheet(pool: &SqlitePool) -> String {
        let id = ulid::Ulid::new().to_string();
        sqlx::query("INSERT INTO worksheets (id, name) VALUES (?, ?)")
            .bind(&id)
            .bind("test")
            .execute(pool)
            .await
            .unwrap();
        id
    }

    // --- parse_file tests ---

    #[test]
    fn test_parse_pdf() {
        let path = format!("{}/test.pdf", FIXTURE_DIR);
        let result = parse_file(&path, "pdf").unwrap();
        assert!(
            result.contains("Hello World"),
            "PDF text should contain 'Hello World', got: {}",
            result
        );
    }

    #[test]
    fn test_parse_docx() {
        let path = format!("{}/test.docx", FIXTURE_DIR);
        let result = parse_file(&path, "docx").unwrap();
        assert!(
            result.contains("Hello World"),
            "DOCX text should contain 'Hello World', got: {}",
            result
        );
    }

    #[test]
    fn test_parse_unsupported_extension() {
        let result = parse_file("test.txt", "txt");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unsupported"));
    }

    #[test]
    fn test_parse_missing_file() {
        let path = format!("{}/nonexistent.pdf", FIXTURE_DIR);
        let result = parse_file(&path, "pdf");
        assert!(result.is_err());
    }

    // --- process_files tests ---

    #[tokio::test]
    async fn test_process_files_empty() {
        let pool = setup_test_db().await;
        let worksheet_id = seed_worksheet(&pool).await;
        let tokenizer = test_tokenizer();

        let result = process_files(&pool, &worksheet_id, &[], &tokenizer)
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_process_files_file_not_found() {
        let pool = setup_test_db().await;
        let worksheet_id = seed_worksheet(&pool).await;
        let tokenizer = test_tokenizer();
        let bad_id = ulid::Ulid::new().to_string();

        let result = process_files(&pool, &worksheet_id, &[bad_id], &tokenizer).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_process_files_unsupported_extension() {
        let pool = setup_test_db().await;
        let worksheet_id = seed_worksheet(&pool).await;
        let tokenizer = test_tokenizer();
        let file_id = ulid::Ulid::new().to_string();

        sqlx::query(
            "INSERT INTO files (id, worksheet_id, path, name, extension, size) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&file_id)
        .bind(&worksheet_id)
        .bind("test.txt")
        .bind("test.txt")
        .bind("txt")
        .bind(0i64)
        .execute(&pool)
        .await
        .unwrap();

        let result = process_files(&pool, &worksheet_id, &[file_id], &tokenizer).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unsupported"));
    }

    #[tokio::test]
    async fn test_process_files_pdf() {
        let pool = setup_test_db().await;
        let worksheet_id = seed_worksheet(&pool).await;
        let tokenizer = test_tokenizer();
        let file_id = ulid::Ulid::new().to_string();
        let pdf_path = format!("{}/test.pdf", FIXTURE_DIR);

        sqlx::query(
            "INSERT INTO files (id, worksheet_id, path, name, extension, size) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&file_id)
        .bind(&worksheet_id)
        .bind(&pdf_path)
        .bind("test.pdf")
        .bind("pdf")
        .bind(618i64)
        .execute(&pool)
        .await
        .unwrap();

        let chunks = process_files(&pool, &worksheet_id, &[file_id.clone()], &tokenizer)
            .await
            .unwrap();

        assert!(!chunks.is_empty(), "Should produce at least one chunk");
        assert!(chunks[0].text.contains("Hello World"));

        // Verify file status updated
        let status: String = sqlx::query_scalar("SELECT status FROM files WHERE id = ?")
            .bind(&file_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "parsed");
    }

    #[tokio::test]
    async fn test_process_files_docx() {
        let pool = setup_test_db().await;
        let worksheet_id = seed_worksheet(&pool).await;
        let tokenizer = test_tokenizer();
        let file_id = ulid::Ulid::new().to_string();
        let docx_path = format!("{}/test.docx", FIXTURE_DIR);

        sqlx::query(
            "INSERT INTO files (id, worksheet_id, path, name, extension, size) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&file_id)
        .bind(&worksheet_id)
        .bind(&docx_path)
        .bind("test.docx")
        .bind("docx")
        .bind(1191i64)
        .execute(&pool)
        .await
        .unwrap();

        let chunks = process_files(&pool, &worksheet_id, &[file_id.clone()], &tokenizer)
            .await
            .unwrap();

        assert!(!chunks.is_empty(), "Should produce at least one chunk");
        assert!(chunks[0].text.contains("Hello World"));

        let status: String = sqlx::query_scalar("SELECT status FROM files WHERE id = ?")
            .bind(&file_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "parsed");
    }
}
