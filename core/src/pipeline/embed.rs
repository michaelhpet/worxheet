use std::sync::Arc;

use sqlx::SqlitePool;

use crate::models::{ModelPool, cpu_threads};

/// How many embedding streams run concurrently.
const EMBED_CONCURRENCY: usize = 4;

/// Serialize a float32 vector into little-endian bytes for a SQLite BLOB.
pub fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Deserialize a float32 vector from a SQLite BLOB.
pub fn bytes_to_embedding(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if !bytes.len().is_multiple_of(4) {
        return Err(format!(
            "Invalid embedding byte length: {} (not a multiple of 4)",
            bytes.len()
        ));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

/// Embed every chunk of a worksheet that does not yet have a vector and store
/// the vectors as BLOBs. Returns the number of chunks embedded.
///
/// Texts are split into contiguous slices embedded on parallel threads, each
/// with a proportional share of the CPU budget. Results are stored in one
/// transaction.
pub async fn embed_missing_chunks(
    pool: &SqlitePool,
    models: &Arc<ModelPool>,
    worksheet_id: &str,
) -> Result<usize, String> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT id, text FROM chunks WHERE worksheet_id = ? AND embedding IS NULL",
    )
    .bind(worksheet_id)
    .fetch_all(pool)
    .await
    .map_err(|_| String::from("Failed to query chunks"))?;

    if rows.is_empty() {
        return Ok(0);
    }

    let ids: Vec<String> = rows.iter().map(|(id, _)| id.clone()).collect();
    let texts: Vec<String> = rows.iter().map(|(_, text)| text.clone()).collect();

    let workers = texts.len().min(EMBED_CONCURRENCY).max(1);
    let slice_len = texts.len().div_ceil(workers);
    let threads_per_worker = (cpu_threads() / workers as i32).max(1);

    let models = models.clone();
    let embeddings = tauri::async_runtime::spawn_blocking(
        move || -> Result<Vec<Vec<f32>>, String> {
            let embedder = models.embedder()?;
            let slices: Vec<Vec<&str>> = texts
                .chunks(slice_len)
                .map(|slice| slice.iter().map(String::as_str).collect())
                .collect();

            let results: Vec<Result<Vec<Vec<f32>>, String>> = std::thread::scope(|scope| {
                let embedder = &embedder;
                let handles: Vec<_> = slices
                    .into_iter()
                    .map(|slice| {
                        scope.spawn(move || {
                            embedder.embed_with_threads(&slice, threads_per_worker)
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|handle| {
                        handle.join().unwrap_or_else(|_| {
                            Err(String::from("Embedding worker thread panicked"))
                        })
                    })
                    .collect()
            });

            let mut output = Vec::with_capacity(texts.len());
            for result in results {
                output.extend(result?);
            }
            Ok(output)
        },
    )
    .await
    .map_err(|e| format!("Embedding task failed: {e}"))??;

    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| String::from("Failed to begin embedding transaction"))?;
    for (id, vector) in ids.iter().zip(&embeddings) {
        let blob = embedding_to_bytes(vector);
        sqlx::query("UPDATE chunks SET embedding = ? WHERE id = ?")
            .bind(blob)
            .bind(id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| String::from("Failed to store embedding"))?;
    }
    transaction
        .commit()
        .await
        .map_err(|_| String::from("Failed to commit stored embeddings"))?;

    Ok(embeddings.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedding_roundtrip() {
        let vector = vec![0.1, -0.5, 3.25, 0.0, 1.0e-6];
        let bytes = embedding_to_bytes(&vector);
        assert_eq!(bytes.len(), vector.len() * 4);
        assert_eq!(bytes_to_embedding(&bytes).unwrap(), vector);
    }

    #[test]
    fn test_bytes_to_embedding_invalid_length() {
        assert!(bytes_to_embedding(&[0, 1, 2]).is_err());
        assert!(bytes_to_embedding(&[]).is_ok());
    }
}