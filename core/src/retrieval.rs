use sqlx::SqlitePool;

use crate::schema::Chunk;

/// Serialize a float32 vector into little-endian bytes for a SQLite BLOB.
pub fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Deserialize a float32 vector from a SQLite BLOB.
pub fn bytes_to_embedding(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if bytes.len() % 4 != 0 {
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

/// Cosine similarity. Embeddings are L2-normalized at creation time, so this is
/// a plain dot product.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Fetch all chunks of a worksheet that have an embedding, score them against
/// the query vector, and return the top `k` by descending similarity.
pub async fn retrieve(
    pool: &SqlitePool,
    worksheet_id: &str,
    query: &[f32],
    k: usize,
) -> Result<Vec<(Chunk, f32)>, String> {
    let rows = sqlx::query_as::<_, (String, String, String, i32, String, Vec<u8>)>(
        "SELECT id, worksheet_id, file_id, position, text, embedding
         FROM chunks
         WHERE worksheet_id = ? AND embedding IS NOT NULL",
    )
    .bind(worksheet_id)
    .fetch_all(pool)
    .await
    .map_err(|_| String::from("Failed to query chunks"))?;

    let mut scored = Vec::with_capacity(rows.len());
    for (id, worksheet_id, file_id, position, text, embedding) in rows {
        let vector = bytes_to_embedding(&embedding)?;
        scored.push((
            Chunk {
                id,
                worksheet_id,
                file_id,
                position,
                text,
                embedding: Some(vector.clone()),
            },
            cosine_similarity(query, &vector),
        ));
    }

    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(k);

    Ok(scored)
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

    #[test]
    fn test_cosine_orders_similar_first() {
        let query = vec![1.0, 0.0];
        let near = vec![0.99, 0.14];
        let far = vec![0.0, 1.0];
        let score_near = cosine_similarity(&query, &near);
        let score_far = cosine_similarity(&query, &far);
        assert!(score_near > score_far);
    }
}
