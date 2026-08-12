use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use ulid::Ulid;

use crate::cluster::{self, MAX_CONTEXT_CHUNKS};
use crate::generation::{
    GenerationParams, items_field_for, schema_for, system_prompt_for, user_message_for,
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

/// Parse, chunk, and store the given files of a worksheet. Emits
/// `ingestion-progress` events as each file completes.
#[tauri::command]
pub async fn process_files(
    state: State<'_, AppState>,
    app: AppHandle,
    worksheet_id: String,
    file_ids: Vec<String>,
) -> Result<Vec<Chunk>, String> {
    run_process_files(Some(&app), &state.database, &state.models, &worksheet_id, &file_ids).await
}

pub async fn run_process_files(
    app: Option<&AppHandle>,
    pool: &sqlx::SqlitePool,
    models: &Arc<ModelPool>,
    worksheet_id: &str,
    file_ids: &[String],
) -> Result<Vec<Chunk>, String> {
    let tokenizer_models = models.clone();
    let tokenizer = tauri::async_runtime::spawn_blocking(move || tokenizer_models.tokenizer())
        .await
        .map_err(|e| format!("Tokenizer task failed: {e}"))??;

    let total = file_ids.len();
    let mut on_progress = |done: usize, _total: usize| {
        if let Some(app) = app {
            let _ = app.emit(
                "ingestion-progress",
                serde_json::json!({
                    "worksheet_id": worksheet_id,
                    "done": done,
                    "total": total,
                }),
            );
        }
    };

    let mut chunks = ingest::process_files(
        pool,
        worksheet_id,
        file_ids,
        &tokenizer,
        Some(&mut on_progress),
    )
    .await?;

    if !chunks.is_empty() {
        embed_missing_chunks(pool, models, worksheet_id).await?;
        rebuild_clusters(pool, worksheet_id).await?;

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
            by_id.insert(chunk_id, retrieval::bytes_to_embedding(&blob)?);
        }
        for chunk in &mut chunks {
            chunk.embedding = by_id.get(&chunk.id).cloned();
        }
    }

    Ok(chunks)
}

/// Embed every chunk of a worksheet that does not yet have a vector and store
/// the vectors as BLOBs. Returns the number of chunks embedded.
async fn embed_missing_chunks(
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

/// Recompute HDBSCAN clusters over every embedded chunk of a worksheet and
/// persist the centroids plus each chunk's cluster assignment.
async fn rebuild_clusters(
    pool: &sqlx::SqlitePool,
    worksheet_id: &str,
) -> Result<(), String> {
    let rows = sqlx::query_as::<_, (String, Vec<u8>)>(
        "SELECT id, embedding
         FROM chunks
         WHERE worksheet_id = ? AND embedding IS NOT NULL
         ORDER BY position",
    )
    .bind(worksheet_id)
    .fetch_all(pool)
    .await
    .map_err(|_| String::from("Failed to query embedded chunks"))?;

    if rows.is_empty() {
        return Ok(());
    }

    let ids: Vec<String> = rows.iter().map(|(id, _)| id.clone()).collect();
    let vectors: Vec<Vec<f32>> = rows
        .iter()
        .map(|(_, blob)| retrieval::bytes_to_embedding(blob))
        .collect::<Result<_, _>>()?;

    let assignment = tauri::async_runtime::spawn_blocking(move || {
        cluster::assign_clusters(&vectors)
    })
    .await
    .map_err(|e| format!("Clustering task failed: {e}"))??;

    let mut sizes = vec![0usize; assignment.centroids.len()];
    for label in &assignment.labels {
        if *label >= 0 {
            sizes[*label as usize] += 1;
        }
    }

    sqlx::query("DELETE FROM clusters WHERE worksheet_id = ?")
        .bind(worksheet_id)
        .execute(pool)
        .await
        .map_err(|_| String::from("Failed to clear old clusters"))?;

    for (cluster_index, centroid) in assignment.centroids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO clusters (worksheet_id, cluster_index, centroid, size)
             VALUES (?, ?, ?, ?)",
        )
        .bind(worksheet_id)
        .bind(cluster_index as i32)
        .bind(retrieval::embedding_to_bytes(centroid))
        .bind(sizes[cluster_index] as i64)
        .execute(pool)
        .await
        .map_err(|_| String::from("Failed to store cluster"))?;
    }

    for (id, label) in ids.iter().zip(&assignment.labels) {
        let index: Option<i32> = (*label >= 0).then_some(*label);
        sqlx::query("UPDATE chunks SET cluster_index = ? WHERE id = ?")
            .bind(index)
            .bind(id)
            .execute(pool)
            .await
            .map_err(|_| String::from("Failed to update chunk cluster"))?;
    }

    Ok(())
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
    embed_missing_chunks(pool, models, worksheet_id).await
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
/// on the fly. Question-style types (MCQ, essay, completion) exhaust the
/// material one unit per topic cluster, each yielding one or more items that
/// are persisted individually. Summary and MindMap produce one worksheet-wide
/// artifact assembled from per-cluster sections in source order. HDBSCAN noise
/// chunks are skipped. Every unit generates with a derived seed so a batch
/// never repeats itself. Emits `generation-progress` events.
#[tauri::command]
pub async fn generate_artifacts(
    state: State<'_, AppState>,
    app: AppHandle,
    worksheet_id: String,
    artifact_type: ArtifactType,
    params: Option<GenerationParams>,
) -> Result<Vec<Artifact>, String> {
    run_generate_artifacts(
        Some(&app),
        &state.database,
        &state.models,
        &worksheet_id,
        &artifact_type,
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
    params: Option<GenerationParams>,
) -> Result<Vec<Artifact>, String> {
    // Clear any stale progress from a previous run before the slow work starts.
    if let Some(app) = app {
        let _ = app.emit(
            "generation-progress",
            serde_json::json!({
                "worksheet_id": worksheet_id,
                "done": 0,
                "total": 0,
            }),
        );
    }

    let params = params.unwrap_or_default();

    let rows = sqlx::query_as::<_, (String, i32, String, Option<i32>, Option<Vec<u8>>)>(
        "SELECT id, position, text, cluster_index, embedding
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

    let at_for_gen = artifact_type.clone();
    let ws_for_gen = worksheet_id.to_string();
    let app = app.cloned();
    let models = models.clone();

    let prepared = tauri::async_runtime::spawn_blocking(move || {
        let embedder = models.embedder()?;

        let mut ids: Vec<String> = Vec::with_capacity(rows.len());
        let mut positions: Vec<i32> = Vec::with_capacity(rows.len());
        let mut texts: Vec<String> = Vec::with_capacity(rows.len());
        let mut labels: Vec<i32> = Vec::with_capacity(rows.len());
        let mut missing: Vec<(String, String)> = Vec::new();

        for (id, position, text, cluster_index, embedding) in rows {
            ids.push(id.clone());
            positions.push(position);
            texts.push(text.clone());
            labels.push(cluster_index.unwrap_or(-1));
            if embedding.is_none() {
                missing.push((id, text));
            }
        }

        let mut new_blobs: Vec<(String, Vec<u8>)> = Vec::new();
        if !missing.is_empty() {
            let refs: Vec<&str> = missing.iter().map(|(_, text)| text.as_str()).collect();
            let vectors = embedder.embed(&refs)?;
            for ((id, _), vector) in missing.iter().zip(&vectors) {
                new_blobs.push((id.clone(), retrieval::embedding_to_bytes(vector)));
            }
        }

        // One generation unit per topic cluster, ordered by source position.
        // Legacy worksheets without any cluster fall back to a single unit that
        // spreads its context evenly across the whole material.
        let units = if labels.iter().any(|label| *label >= 0) {
            cluster::cluster_contexts(&labels, &positions, MAX_CONTEXT_CHUNKS)
        } else {
            let all: Vec<usize> = (0..labels.len()).collect();
            vec![cluster::pick_evenly(&all, MAX_CONTEXT_CHUNKS)]
        };

        if units.is_empty() {
            return Err(String::from("No topic clusters found for this worksheet."));
        }

        let mut contexts: Vec<String> = Vec::with_capacity(units.len());
        let mut unit_sources: Vec<String> = Vec::with_capacity(units.len());
        for mut picked in units {
            picked.sort_by(|a, b| positions[*a].cmp(&positions[*b]));
            let mut context = String::new();
            let mut chunk_ids: Vec<String> = Vec::with_capacity(picked.len());
            for &index in &picked {
                chunk_ids.push(ids[index].clone());
                context.push_str(&texts[index]);
                context.push('\n');
            }
            contexts.push(context);
            unit_sources.push(chunk_ids.join(","));
        }

        let total = contexts.len();
        if let Some(app) = &app {
            let _ = app.emit(
                "generation-progress",
                serde_json::json!({
                    "worksheet_id": ws_for_gen,
                    "done": 0,
                    "total": total,
                }),
            );
        }

        let generator = models.generator()?;
        let system = system_prompt_for(&at_for_gen);
        let schema = schema_for(&at_for_gen);

        // Generate per unit on a best-effort basis: a unit that truncates or
        // returns invalid output is retried once with a doubled token budget,
        // then skipped if it still fails, so one bad cluster never discards an
        // otherwise healthy batch.
        let mut failures: Vec<String> = Vec::new();
        let mut outputs: Vec<String> = Vec::with_capacity(total);
        let mut sources: Vec<String> = Vec::with_capacity(total);

        for (index, context) in contexts.into_iter().enumerate() {
            let attempt = (|| -> Result<String, String> {
                let mut unit_params = params.clone();
                unit_params.seed = params.seed.wrapping_add(index as u32);
                let user = user_message_for(&at_for_gen, &context);
                let prompt = generator.apply_chat_template(system, &user)?;

                let mut output = generator.generate(&prompt, Some(schema), &unit_params)?;
                if !output_parses(&at_for_gen, &output) {
                    let mut retry_params = unit_params.clone();
                    retry_params.max_tokens = unit_params
                        .max_tokens
                        .max(256)
                        .saturating_mul(2)
                        .min(4096);
                    output = generator.generate(&prompt, Some(schema), &retry_params)?;
                }
                if !output_parses(&at_for_gen, &output) {
                    return Err(format!("Model returned invalid JSON: {output}"));
                }
                Ok(output)
            })();

            match attempt {
                Ok(output) => {
                    outputs.push(output);
                    sources.push(unit_sources[index].clone());
                }
                Err(error) => failures.push(format!("unit {}: {error}", index + 1)),
            }

            if let Some(app) = &app {
                let _ = app.emit(
                    "generation-progress",
                    serde_json::json!({
                        "worksheet_id": ws_for_gen,
                        "done": index + 1,
                        "total": total,
                    }),
                );
            }
        }

        if outputs.is_empty() {
            let detail = failures
                .first()
                .cloned()
                .unwrap_or_else(|| String::from("no units produced output"));
            return Err(format!(
                "All {total} generation units failed; first error: {detail}"
            ));
        }

        let pending = assemble_artifacts(&at_for_gen, outputs, &sources)?;

        Ok::<_, String>((pending, new_blobs))
    })
    .await
    .map_err(|e| format!("Generation task failed: {e}"))??;

    let (pending, new_blobs) = prepared;

    for (id, blob) in &new_blobs {
        sqlx::query("UPDATE chunks SET embedding = ? WHERE id = ?")
            .bind(blob.clone())
            .bind(id)
            .execute(pool)
            .await
            .map_err(|_| String::from("Failed to store embedding"))?;
    }

    let mut artifacts = Vec::with_capacity(pending.len());
    for (source, content) in pending {
        let artifact = Artifact {
            id: Ulid::new().to_string(),
            worksheet_id: worksheet_id.to_string(),
            artifact_type: artifact_type.clone(),
            source,
            content,
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

/// Whether a model output is valid JSON and, for question-style types, carries
/// its item array. Used to decide whether a unit needs a wider-budget retry.
fn output_parses(artifact_type: &ArtifactType, output: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(output) else {
        return false;
    };
    match items_field_for(artifact_type) {
        Some(field) => value
            .get(field)
            .and_then(serde_json::Value::as_array)
            .is_some(),
        None => true,
    }
}

/// Turn per-unit generation outputs into `(source, content)` artifact records.
/// Question-style types split each unit's item array into one artifact per
/// item; Summary and MindMap merge every unit into a single worksheet-wide
/// artifact ordered by source position.
fn assemble_artifacts(
    artifact_type: &ArtifactType,
    outputs: Vec<String>,
    unit_sources: &[String],
) -> Result<Vec<(String, String)>, String> {
    let parse_unit = |output: &str| -> Result<serde_json::Value, String> {
        serde_json::from_str(output).map_err(|error| {
            format!(
                "Model returned invalid JSON for {}: {error}; got: {output}",
                artifact_type.to_db()
            )
        })
    };

    if let Some(field) = items_field_for(artifact_type) {
        let mut pending = Vec::new();
        for (output, source) in outputs.iter().zip(unit_sources) {
            let value = parse_unit(output)?;
            let items = value
                .get(field)
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| {
                    format!(
                        "Model output for {} is missing the \"{field}\" array; got: {output}",
                        artifact_type.to_db()
                    )
                })?;
            for item in items {
                pending.push((source.clone(), item.to_string()));
            }
        }
        return Ok(pending);
    }

    let mut title = String::new();
    let mut topic = String::new();
    let mut summaries: Vec<String> = Vec::new();
    let mut key_points: Vec<String> = Vec::new();
    let mut branches: Vec<serde_json::Value> = Vec::new();

    for output in &outputs {
        let value = parse_unit(output)?;
        match artifact_type {
            ArtifactType::Summary => {
                if title.is_empty() {
                    title = value
                        .get("title")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("Worksheet summary")
                        .to_string();
                }
                if let Some(summary) = value.get("summary").and_then(serde_json::Value::as_str)
                {
                    summaries.push(summary.trim().to_string());
                }
                if let Some(points) = value.get("key_points").and_then(serde_json::Value::as_array)
                {
                    key_points.extend(
                        points
                            .iter()
                            .filter_map(|point| point.as_str().map(ToString::to_string)),
                    );
                }
            }
            ArtifactType::MindMap => {
                if topic.is_empty() {
                    topic = value
                        .get("topic")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("Overview")
                        .to_string();
                }
                if let Some(unit_branches) = value
                    .get("branches")
                    .and_then(serde_json::Value::as_array)
                {
                    branches.extend(unit_branches.iter().cloned());
                }
            }
            _ => unreachable!("items_field_for covers every question-style type"),
        }
    }

    let content = match artifact_type {
        ArtifactType::Summary => serde_json::json!({
            "title": title,
            "summary": summaries.join("\n\n"),
            "key_points": key_points,
        }),
        ArtifactType::MindMap => serde_json::json!({
            "topic": topic,
            "branches": branches,
        }),
        _ => unreachable!("items_field_for covers every question-style type"),
    }
    .to_string();

    Ok(vec![(unit_sources.join(","), content)])
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

        let chunks = run_process_files(None, &pool, &models, &worksheet_id, &[file_id])
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

        let embedded = run_embed_worksheet(&pool, &models, &worksheet_id)
            .await
            .expect("embed_worksheet should succeed");
        assert_eq!(embedded, 0, "no work left after processing");

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
        run_process_files(None, &pool, &models, &worksheet_id, &[file_id])
            .await
            .expect("process_files should succeed");

        let params = GenerationParams {
            temperature: 0.3,
            max_tokens: 1024,
            ..Default::default()
        };
        let artifacts = run_generate_artifacts(
            None,
            &pool,
            &models,
            &worksheet_id,
            &ArtifactType::MultipleChoiceQuiz,
            Some(params),
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
            let blob = retrieval::embedding_to_bytes(&vector);
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

        let params = GenerationParams {
            temperature: 0.3,
            max_tokens: 1024,
            ..Default::default()
        };
        let artifacts = run_generate_artifacts(
            None,
            &pool,
            &models,
            &worksheet_id,
            &ArtifactType::CompletionQuiz,
            Some(params),
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

        let params = GenerationParams {
            temperature: 0.3,
            max_tokens: 1024,
            ..Default::default()
        };

        let summaries = run_generate_artifacts(
            None,
            &pool,
            &models,
            &worksheet_id,
            &ArtifactType::Summary,
            Some(params.clone()),
        )
        .await
        .expect("summary generation should succeed");
        assert_eq!(summaries.len(), 1, "summary is one worksheet-wide artifact");
        let summary_value: serde_json::Value = serde_json::from_str(&summaries[0].content)
            .expect("summary content should be valid JSON");
        assert!(summary_value.get("summary").is_some());
        assert!(summary_value.get("key_points").is_some());

        let mind_maps = run_generate_artifacts(
            None,
            &pool,
            &models,
            &worksheet_id,
            &ArtifactType::MindMap,
            Some(params),
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
