use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use ulid::Ulid;

use crate::generation::{
    GenerationParams, schema_for, system_prompt_for, user_message_for,
};
use crate::ingest;
use crate::models::ModelPool;
use crate::retrieval;
use crate::schema::{Artifact, ArtifactType, Chunk};
use crate::AppState;

#[derive(Serialize)]
pub struct FileInfo {
    pub id: String,
    pub name: String,
    pub extension: String,
    pub size: i64,
    pub status: String,
}

#[derive(Serialize)]
pub struct RetrievedChunk {
    pub id: String,
    pub text: String,
    pub position: i32,
    pub score: f32,
}

/// List the files attached to a worksheet (used by the frontend to obtain file
/// IDs before running the ingestion pipeline).
#[tauri::command]
pub async fn get_files(
    state: State<'_, AppState>,
    worksheet_id: String,
) -> Result<Vec<FileInfo>, String> {
    let pool = state.database.clone();
    let rows = sqlx::query_as::<_, (String, String, String, i64, String)>(
        "SELECT id, name, extension, size, status
         FROM files
         WHERE worksheet_id = ?
         ORDER BY created_at",
    )
    .bind(&worksheet_id)
    .fetch_all(&pool)
    .await
    .map_err(|_| String::from("Failed to fetch files"))?;

    Ok(rows
        .into_iter()
        .map(|(id, name, extension, size, status)| FileInfo {
            id,
            name,
            extension,
            size,
            status,
        })
        .collect())
}

/// Parse, chunk, and store the given files of a worksheet.
#[tauri::command]
pub async fn process_files(
    state: State<'_, AppState>,
    worksheet_id: String,
    file_ids: Vec<String>,
) -> Result<Vec<Chunk>, String> {
    run_process_files(&state.database, &state.models, &worksheet_id, &file_ids).await
}

pub async fn run_process_files(
    pool: &sqlx::SqlitePool,
    models: &Arc<ModelPool>,
    worksheet_id: &str,
    file_ids: &[String],
) -> Result<Vec<Chunk>, String> {
    let models = models.clone();
    let tokenizer = tauri::async_runtime::spawn_blocking(move || models.tokenizer())
        .await
        .map_err(|e| format!("Tokenizer task failed: {e}"))??;

    ingest::process_files(pool, worksheet_id, file_ids, &tokenizer).await
}

/// Embed every chunk of a worksheet that does not yet have an embedding and
/// store the vectors as BLOBs. Returns the number of chunks embedded.
#[tauri::command]
pub async fn embed_worksheet(
    state: State<'_, AppState>,
    worksheet_id: String,
) -> Result<usize, String> {
    run_embed_worksheet(&state.database, &state.models, &worksheet_id).await
}

pub async fn run_embed_worksheet(
    pool: &sqlx::SqlitePool,
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

    let models = models.clone();
    let embeddings = tauri::async_runtime::spawn_blocking(move || {
        let embedder = models.embedder()?;
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        embedder.embed(&refs)
    })
    .await
    .map_err(|e| format!("Embedding task failed: {e}"))??;

    for (id, vector) in ids.iter().zip(&embeddings) {
        let blob = retrieval::embedding_to_bytes(vector);
        sqlx::query("UPDATE chunks SET embedding = ? WHERE id = ?")
            .bind(blob)
            .bind(id)
            .execute(pool)
            .await
            .map_err(|_| String::from("Failed to store embedding"))?;
    }

    Ok(embeddings.len())
}

/// RAG retrieval: embed the query and return the top-k most similar chunks.
#[tauri::command]
pub async fn retrieve_chunks(
    state: State<'_, AppState>,
    worksheet_id: String,
    query: String,
    top_k: Option<usize>,
) -> Result<Vec<RetrievedChunk>, String> {
    run_retrieve_chunks(&state.database, &state.models, &worksheet_id, &query, top_k).await
}

pub async fn run_retrieve_chunks(
    pool: &sqlx::SqlitePool,
    models: &Arc<ModelPool>,
    worksheet_id: &str,
    query: &str,
    top_k: Option<usize>,
) -> Result<Vec<RetrievedChunk>, String> {
    let k = top_k.unwrap_or(8).clamp(1, 50);
    let models = models.clone();
    let query = query.to_string();
    let query_vector = tauri::async_runtime::spawn_blocking(move || {
        let embedder = models.embedder()?;
        let vectors = embedder.embed(&[query.as_str()])?;
        vectors
            .into_iter()
            .next()
            .ok_or_else(|| String::from("Embedder returned no vector"))
    })
    .await
    .map_err(|e| format!("Embedding task failed: {e}"))??;

    let results = retrieval::retrieve(pool, worksheet_id, &query_vector, k).await?;

    Ok(results
        .into_iter()
        .map(|(chunk, score)| RetrievedChunk {
            id: chunk.id,
            text: chunk.text,
            position: chunk.position,
            score,
        })
        .collect())
}

/// Generate artifacts for a worksheet. Chunks missing embeddings are embedded
/// on the fly; the artifact-specific task is embedded and used to retrieve the
/// top-k relevant chunks as context. Each generated artifact is validated,
/// persisted, and returned. Emits `generation-progress` events.
#[tauri::command]
pub async fn generate_artifacts(
    state: State<'_, AppState>,
    app: AppHandle,
    worksheet_id: String,
    artifact_type: ArtifactType,
    count: Option<usize>,
    params: Option<GenerationParams>,
) -> Result<Vec<Artifact>, String> {
    run_generate_artifacts(
        Some(&app),
        &state.database,
        &state.models,
        &worksheet_id,
        &artifact_type,
        count,
        params,
    )
    .await
}

pub async fn run_generate_artifacts(
    app: Option<&AppHandle>,
    pool: &sqlx::SqlitePool,
    models: &Arc<ModelPool>,
    worksheet_id: &str,
    artifact_type: &ArtifactType,
    count: Option<usize>,
    params: Option<GenerationParams>,
) -> Result<Vec<Artifact>, String> {
    let total = count.unwrap_or(1).clamp(1, 10);
    let params = params.unwrap_or_default();

    let rows = sqlx::query_as::<_, (String, i32, String, Option<Vec<u8>>)>(
        "SELECT id, position, text, embedding
         FROM chunks
         WHERE worksheet_id = ?
         ORDER BY position",
    )
    .bind(worksheet_id)
    .fetch_all(pool)
    .await
    .map_err(|_| String::from("Failed to query chunks"))?;

    if rows.is_empty() {
        return Err(String::from(
            "No chunks found for this worksheet. Run process_files first.",
        ));
    }

    let task_query = task_query_for(artifact_type);
    let at_for_gen = artifact_type.clone();
    let ws_for_gen = worksheet_id.to_string();
    let app = app.cloned();
    let models = models.clone();

    let prepared = tauri::async_runtime::spawn_blocking(move || {
        let embedder = models.embedder()?;

        let mut ids: Vec<String> = Vec::with_capacity(rows.len());
        let mut texts: Vec<String> = Vec::with_capacity(rows.len());
        let mut had_embedding: Vec<bool> = Vec::with_capacity(rows.len());
        let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(rows.len());
        let mut need_text: Vec<String> = Vec::new();

        for (id, _position, text, embedding) in rows {
            ids.push(id.clone());
            texts.push(text.clone());
            match embedding {
                Some(bytes) => {
                    had_embedding.push(true);
                    vectors.push(retrieval::bytes_to_embedding(&bytes)?);
                }
                None => {
                    had_embedding.push(false);
                    vectors.push(Vec::new());
                    need_text.push(text);
                }
            }
        }

        let new_vectors = if need_text.is_empty() {
            Vec::new()
        } else {
            let refs: Vec<&str> = need_text.iter().map(String::as_str).collect();
            embedder.embed(&refs)?
        };
        let mut new_iter = new_vectors.into_iter();
        let mut new_blobs: Vec<(String, Vec<u8>)> = Vec::new();
        for (i, needs_embedding) in had_embedding.iter().enumerate() {
            if !needs_embedding {
                let vector = new_iter
                    .next()
                    .ok_or_else(|| String::from("Embedding count mismatch"))?;
                vectors[i] = vector.clone();
                new_blobs.push((ids[i].clone(), retrieval::embedding_to_bytes(&vector)));
            }
        }

        let query_vector = embedder
            .embed(&[task_query])?
            .into_iter()
            .next()
            .ok_or_else(|| String::from("Embedder returned no vector"))?;

        let mut scored: Vec<(usize, f32)> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| (i, retrieval::cosine_similarity(&query_vector, v)))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let top_k = std::cmp::min(std::cmp::max(total * 3, 3), scored.len());
        let mut context = String::new();
        let mut top_ids: Vec<String> = Vec::new();
        for (i, _) in &scored[..top_k] {
            top_ids.push(ids[*i].clone());
            context.push_str(&texts[*i]);
            context.push('\n');
        }

        let generator = models.generator()?;
        let system = system_prompt_for(&at_for_gen);
        let user = user_message_for(&at_for_gen, &context);
        let prompt = generator.apply_chat_template(system, &user)?;
        let schema = schema_for(&at_for_gen);

        let mut outputs = Vec::with_capacity(total);
        for i in 0..total {
            outputs.push(generator.generate(&prompt, Some(schema), &params)?);
            if let Some(app) = &app {
                let _ = app.emit(
                    "generation-progress",
                    serde_json::json!({
                        "worksheet_id": ws_for_gen,
                        "done": i + 1,
                        "total": total,
                    }),
                );
            }
        }

        Ok::<_, String>((outputs, new_blobs, top_ids))
    })
    .await
    .map_err(|e| format!("Generation task failed: {e}"))??;

    let (outputs, new_blobs, top_ids) = prepared;

    for (id, blob) in &new_blobs {
        sqlx::query("UPDATE chunks SET embedding = ? WHERE id = ?")
            .bind(blob.clone())
            .bind(id)
            .execute(pool)
            .await
            .map_err(|_| String::from("Failed to store embedding"))?;
    }

    let source = top_ids.join(",");
    let mut artifacts = Vec::with_capacity(outputs.len());
    for output in outputs {
        serde_json::from_str::<serde_json::Value>(&output).map_err(|e| {
            format!(
                "Model returned invalid JSON for {}: {e}; got: {output}",
                artifact_type.to_db()
            )
        })?;

        let artifact = Artifact {
            id: Ulid::new().to_string(),
            worksheet_id: worksheet_id.to_string(),
            artifact_type: artifact_type.clone(),
            source: source.clone(),
            content: output,
        };
        sqlx::query(
            "INSERT INTO artifacts (id, worksheet_id, artifact_type, source, content)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&artifact.id)
        .bind(worksheet_id)
        .bind(artifact_type.to_db())
        .bind(&artifact.source)
        .bind(&artifact.content)
        .execute(pool)
        .await
        .map_err(|_| String::from("Failed to persist artifact"))?;

        artifacts.push(artifact);
    }

    Ok(artifacts)
}

/// List the persisted artifacts of a worksheet.
#[tauri::command]
pub async fn get_artifacts(
    state: State<'_, AppState>,
    worksheet_id: String,
) -> Result<Vec<Artifact>, String> {
    let pool = state.database.clone();
    let rows = sqlx::query_as::<_, (String, String, String, String, String)>(
        "SELECT id, worksheet_id, artifact_type, source, content
         FROM artifacts
         WHERE worksheet_id = ?
         ORDER BY created_at",
    )
    .bind(&worksheet_id)
    .fetch_all(&pool)
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

/// Short query used to embed a worksheet against its chunks for retrieval.
fn task_query_for(artifact_type: &ArtifactType) -> &'static str {
    match artifact_type {
        ArtifactType::MultipleChoiceQuiz => "multiple choice quiz question",
        ArtifactType::EssayQuiz => "essay question",
        ArtifactType::CompletionQuiz => "fill in the blank completion",
        ArtifactType::Summary => "summary of key ideas",
        ArtifactType::MindMap => "mind map of concepts and relationships",
    }
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
    async fn test_process_files_and_embed_and_retrieve() {
        let Some(models) = shared_models() else {
            return;
        };
        let pool = setup_db().await;
        let (worksheet_id, file_id) = seed_worksheet_and_file(&pool).await;

        let chunks = run_process_files(&pool, &models, &worksheet_id, &[file_id])
            .await
            .expect("process_files should succeed");
        assert!(!chunks.is_empty());

        let embedded = run_embed_worksheet(&pool, &models, &worksheet_id)
            .await
            .expect("embed_worksheet should succeed");
        assert_eq!(embedded, chunks.len());

        let results =
            run_retrieve_chunks(&pool, &models, &worksheet_id, "cell biology", Some(3))
                .await
                .expect("retrieve should succeed");
        assert!(!results.is_empty());
        assert!(results.len() <= 3);
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

        let err = run_generate_artifacts(
            None,
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
        run_process_files(&pool, &models, &worksheet_id, &[file_id])
            .await
            .expect("process_files should succeed");

        let params = GenerationParams {
            temperature: 0.3,
            max_tokens: 256,
            ..Default::default()
        };
        let artifacts = run_generate_artifacts(
            None,
            &pool,
            &models,
            &worksheet_id,
            &ArtifactType::MultipleChoiceQuiz,
            Some(1),
            Some(params),
        )
        .await
        .expect("generate should succeed");

        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].artifact_type, ArtifactType::MultipleChoiceQuiz);
        let value: serde_json::Value = serde_json::from_str(&artifacts[0].content)
            .expect("artifact content should be valid JSON");
        assert!(value.get("question").is_some());
        assert_eq!(value["options"].as_array().map(Vec::len), Some(4));
    }
}
