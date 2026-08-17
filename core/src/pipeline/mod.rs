use std::sync::Arc;

use sqlx::SqlitePool;

use crate::models::ModelPool;
use crate::schema::{Artifact, ArtifactType, Chunk};

pub mod cluster;
pub mod embed;
pub mod generate;
pub mod ingest;
pub mod jobs;

pub use embed::bytes_to_embedding;
pub use generate::{ProgressFn, generate_artifacts};
pub use jobs::{PipelineJobs, get_status, remove_job, resume_stale, start_job};

/// Parse, chunk, embed, and cluster every file of a worksheet. Reports progress
/// as each file completes through `on_progress` if provided.
pub async fn process_files(
    pool: &SqlitePool,
    models: &Arc<ModelPool>,
    worksheet_id: &str,
    file_ids: &[String],
    on_progress: Option<ProgressFn>,
) -> Result<Vec<Chunk>, String> {
    let tokenizer_models = models.clone();
    let tokenizer = tauri::async_runtime::spawn_blocking(move || tokenizer_models.tokenizer())
        .await
        .map_err(|e| format!("Tokenizer task failed: {e}"))??;

    let total = file_ids.len();
    let mut progress = |done: usize, _total: usize| {
        if let Some(on_progress) = &on_progress {
            on_progress(done, total);
        }
    };

    let mut chunks = ingest::process_files(
        pool,
        worksheet_id,
        file_ids,
        &tokenizer,
        Some(&mut progress),
    )
    .await?;

    if !chunks.is_empty() {
        embed::embed_missing_chunks(pool, models, worksheet_id).await?;
        cluster::rebuild_clusters(pool, worksheet_id).await?;

        // Refresh the in-memory chunks with the vectors we just stored so the
        // returned contract matches the database.
        let rows = sqlx::query_as::<_, (String, Vec<u8>)>(
            "SELECT id, embedding
             FROM chunks
             WHERE worksheet_id = ? AND embedding IS NOT NULL",
        )
        .bind(worksheet_id)
        .fetch_all(pool)
        .await
        .map_err(|_| String::from("Failed to fetch stored embeddings"))?;

        let mut by_id = std::collections::HashMap::new();
        for (chunk_id, blob) in rows {
            by_id.insert(chunk_id, bytes_to_embedding(&blob)?);
        }
        for chunk in &mut chunks {
            chunk.embedding = by_id.get(&chunk.id).cloned();
        }
    }

    Ok(chunks)
}

/// List the persisted artifacts of a worksheet.
pub async fn get_artifacts(pool: &SqlitePool, worksheet_id: &str) -> Result<Vec<Artifact>, String> {
    let rows = sqlx::query_as::<_, (String, String, String, String, String)>(
        "SELECT id, worksheet_id, artifact_type, source, content
         FROM artifacts
         WHERE worksheet_id = ?
         ORDER BY created_at",
    )
    .bind(worksheet_id)
    .fetch_all(pool)
    .await
    .map_err(|_| String::from("Failed to fetch artifacts"))?;

    rows.into_iter()
        .map(|(id, worksheet_id, artifact_type, source, content)| {
            Ok(Artifact {
                id,
                worksheet_id,
                artifact_type: ArtifactType::from_db(&artifact_type)?,
                source,
                content,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

    fn models_dir() -> PathBuf {
        std::env::var("WORXHEET_MODELS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/models")))
    }

    fn shared_models() -> Option<Arc<ModelPool>> {
        static MODELS: OnceLock<Option<Arc<ModelPool>>> = OnceLock::new();
        MODELS
            .get_or_init(|| {
                let dir = models_dir();
                if !dir.join("bge-small-en-v1.5-q8_0.gguf").exists()
                    || !dir.join("smollm2-360m-instruct-q8_0.gguf").exists()
                    || !dir.join("tokenizer.json").exists()
                {
                    eprintln!("skipping: models not found in {}", dir.display());
                    return None;
                }
                Some(Arc::new(ModelPool::new(dir)))
            })
            .clone()
    }

    async fn setup_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("Failed to create in-memory pool");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("Failed to run migrations");
        pool
    }

    async fn seed_worksheet_and_file(pool: &SqlitePool) -> (String, String) {
        let worksheet_id = ulid::Ulid::new().to_string();
        sqlx::query("INSERT INTO worksheets (id, name) VALUES (?, ?)")
            .bind(&worksheet_id)
            .bind("test")
            .execute(pool)
            .await
            .unwrap();

        let file_id = ulid::Ulid::new().to_string();
        let pdf = format!("{}/test.pdf", FIXTURE_DIR);
        sqlx::query(
            "INSERT INTO files (id, worksheet_id, path, name, extension, size)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&file_id)
        .bind(&worksheet_id)
        .bind(&pdf)
        .bind("test.pdf")
        .bind("pdf")
        .bind(618i64)
        .execute(pool)
        .await
        .unwrap();

        (worksheet_id, file_id)
    }

    #[tokio::test]
    async fn test_process_files_embeds_and_clusters() {
        let Some(models) = shared_models() else {
            return;
        };
        let pool = setup_db().await;
        let (worksheet_id, file_id) = seed_worksheet_and_file(&pool).await;

        let chunks = process_files(&pool, &models, &worksheet_id, &[file_id], None)
            .await
            .expect("process_files should succeed");
        assert!(!chunks.is_empty());

        // Processing now embeds and clusters, so every chunk has a vector and
        // the worksheet has persisted clusters before any generation happens.
        let unembedded: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM chunks WHERE worksheet_id = ? AND embedding IS NULL")
                .bind(&worksheet_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(unembedded, 0);

        let cluster_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM clusters WHERE worksheet_id = ?")
            .bind(&worksheet_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(cluster_count >= 1, "processing should persist clusters");
    }

    #[tokio::test]
    async fn test_generate_requires_chunks() {
        let Some(models) = shared_models() else {
            return;
        };
        let pool = setup_db().await;
        let worksheet_id = ulid::Ulid::new().to_string();
        sqlx::query("INSERT INTO worksheets (id, name) VALUES (?, ?)")
            .bind(&worksheet_id)
            .bind("test")
            .execute(&pool)
            .await
            .unwrap();

        let err = generate_artifacts(
            &pool,
            &models,
            &worksheet_id,
            &ArtifactType::Summary,
            None,
            None,
        )
        .await
        .expect_err("should error without chunks");
        assert!(err.contains("No chunks"));
    }

    #[tokio::test]
    async fn test_generate_mcq_artifact() {
        let Some(models) = shared_models() else {
            return;
        };
        let pool = setup_db().await;
        let (worksheet_id, file_id) = seed_worksheet_and_file(&pool).await;
        process_files(&pool, &models, &worksheet_id, &[file_id], None)
            .await
            .expect("process_files should succeed");

        let params = crate::models::GenerationParams {
            temperature: 0.3,
            max_tokens: 1024,
            ..Default::default()
        };
        let artifacts = generate_artifacts(
            &pool,
            &models,
            &worksheet_id,
            &ArtifactType::MultipleChoiceQuiz,
            Some(params),
            None,
        )
        .await
        .expect("generate should succeed");

        assert!(!artifacts.is_empty(), "exhaustive generation must yield artifacts");
        for artifact in &artifacts {
            assert_eq!(artifact.artifact_type, ArtifactType::MultipleChoiceQuiz);
            let value: serde_json::Value = serde_json::from_str(&artifact.content)
                .expect("artifact content should be valid JSON");
            assert!(value.get("question").is_some());
            assert_eq!(value["options"].as_array().map(Vec::len), Some(4));
        }
    }

    async fn seed_chunks(
        pool: &SqlitePool,
        worksheet_id: &str,
        file_id: &str,
        count: usize,
    ) {
        let passage = concat!(
            "The mitochondrion is the powerhouse of the cell, where respiration ",
            "proceeds through glycolysis and the citric acid cycle. ",
        );
        let text = passage.repeat(16);
        for index in 0..count {
            let chunk_id = ulid::Ulid::new().to_string();
            let vector: Vec<f32> = (0..384)
                .map(|dimension| ((index + dimension) as f32 % 17.0) / 17.0)
                .collect();
            let blob = embed::embedding_to_bytes(&vector);
            sqlx::query(
                "INSERT INTO chunks (id, worksheet_id, file_id, position, text, embedding, cluster_index)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&chunk_id)
            .bind(worksheet_id)
            .bind(file_id)
            .bind(index as i32)
            .bind(&text)
            .bind(blob)
            .bind((index % 3) as i32)
            .execute(pool)
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn test_generate_exhausts_clusters() {
        let Some(models) = shared_models() else {
            return;
        };
        let pool = setup_db().await;
        let (worksheet_id, file_id) = seed_worksheet_and_file(&pool).await;
        seed_chunks(&pool, &worksheet_id, &file_id, 20).await;

        let params = crate::models::GenerationParams {
            temperature: 0.3,
            max_tokens: 1024,
            ..Default::default()
        };
        let artifacts = generate_artifacts(
            &pool,
            &models,
            &worksheet_id,
            &ArtifactType::CompletionQuiz,
            Some(params),
            None,
        )
        .await
        .expect("exhaustive generation should succeed");

        assert!(
            artifacts.len() >= 3,
            "each of the 3 seeded clusters must yield at least one artifact, got {}",
            artifacts.len()
        );
        for artifact in &artifacts {
            let value: serde_json::Value = serde_json::from_str(&artifact.content)
                .expect("artifact content should be valid JSON");
            assert!(value.get("sentence").is_some());
            assert!(value.get("answer").is_some());
        }
    }

    #[tokio::test]
    async fn test_generate_summary_and_mindmap_single_artifact() {
        let Some(models) = shared_models() else {
            return;
        };
        let pool = setup_db().await;
        let (worksheet_id, file_id) = seed_worksheet_and_file(&pool).await;
        seed_chunks(&pool, &worksheet_id, &file_id, 20).await;

        let params = crate::models::GenerationParams {
            temperature: 0.3,
            max_tokens: 1024,
            ..Default::default()
        };

        let summaries = generate_artifacts(
            &pool,
            &models,
            &worksheet_id,
            &ArtifactType::Summary,
            Some(params.clone()),
            None,
        )
        .await
        .expect("summary generation should succeed");
        assert_eq!(summaries.len(), 1, "summary is one worksheet-wide artifact");
        let summary_value: serde_json::Value = serde_json::from_str(&summaries[0].content)
            .expect("summary content should be valid JSON");
        assert!(summary_value.get("summary").is_some());
        assert!(summary_value.get("key_points").is_some());

        let mind_maps = generate_artifacts(
            &pool,
            &models,
            &worksheet_id,
            &ArtifactType::MindMap,
            Some(params),
            None,
        )
        .await
        .expect("mind map generation should succeed");
        assert_eq!(mind_maps.len(), 1, "mind map is one worksheet-wide artifact");
        let mind_map_value: serde_json::Value = serde_json::from_str(&mind_maps[0].content)
            .expect("mind map content should be valid JSON");
        assert!(mind_map_value.get("topic").is_some());
        assert!(mind_map_value.get("branches").is_some());
    }
}