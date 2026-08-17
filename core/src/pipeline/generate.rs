use std::sync::Arc;

use sqlx::SqlitePool;
use ulid::Ulid;

use super::cluster::{self, MAX_CONTEXT_CHUNKS};
use super::embed::embedding_to_bytes;
use crate::models::ModelPool;
use crate::schema::{Artifact, ArtifactType};

pub use crate::models::GenerationParams;

/// Progress callback invoked with `(units_done, units_total)` during generation.
pub type ProgressFn = Arc<dyn Fn(usize, usize) + Send + Sync>;

/// Generate artifacts for a worksheet. Chunks missing embeddings are embedded
/// on the fly. Question-style types (MCQ, essay, completion) exhaust the
/// material one unit per topic cluster, each yielding one or more items that
/// are persisted individually. Summary and MindMap produce one worksheet-wide
/// artifact assembled from per-cluster sections in source order. HDBSCAN noise
/// chunks are skipped. Every unit generates with a derived seed so a batch
/// never repeats itself. Reports per-unit progress through `on_progress`.
pub async fn generate_artifacts(
    pool: &SqlitePool,
    models: &Arc<ModelPool>,
    worksheet_id: &str,
    artifact_type: &ArtifactType,
    params: Option<GenerationParams>,
    on_progress: Option<ProgressFn>,
) -> Result<Vec<Artifact>, String> {
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
    let models = models.clone();
    let on_progress = on_progress.clone();

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
                new_blobs.push((id.clone(), embedding_to_bytes(vector)));
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
        if let Some(on_progress) = &on_progress {
            on_progress(0, total);
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
                    retry_params.max_tokens =
                        unit_params.max_tokens.max(256).saturating_mul(2).min(4096);
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

            if let Some(on_progress) = &on_progress {
                on_progress(index + 1, total);
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
                if let Some(summary) = value.get("summary").and_then(serde_json::Value::as_str) {
                    summaries.push(summary.trim().to_string());
                }
                if let Some(points) = value
                    .get("key_points")
                    .and_then(serde_json::Value::as_array)
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
                if let Some(unit_branches) =
                    value.get("branches").and_then(serde_json::Value::as_array)
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

/// The JSON array field holding per-cluster items for a question-style artifact
/// type. Whole-worksheet types (Summary, MindMap) return `None`.
pub fn items_field_for(artifact_type: &ArtifactType) -> Option<&'static str> {
    match artifact_type {
        ArtifactType::MultipleChoiceQuiz | ArtifactType::EssayQuiz => Some("questions"),
        ArtifactType::CompletionQuiz => Some("items"),
        ArtifactType::Summary | ArtifactType::MindMap => None,
    }
}

/// JSON schema constraining the generated artifact for a given type.
///
/// Question-style types wrap their items in an array so a single cluster can
/// yield one or more questions; every item is persisted as its own artifact.
/// Summary and MindMap use a single object per cluster, which the pipeline
/// concatenates into one worksheet-wide artifact.
pub fn schema_for(artifact_type: &ArtifactType) -> &'static str {
    match artifact_type {
        ArtifactType::MultipleChoiceQuiz => {
            r#"{
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 8,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string" },
                            "options": { "type": "array", "items": { "type": "string" }, "minItems": 4, "maxItems": 4 },
                            "answer": { "type": "integer" },
                            "explanation": { "type": "string" }
                        },
                        "required": ["question", "options", "answer", "explanation"]
                    }
                }
            },
            "required": ["questions"]
        }"#
        }
        ArtifactType::EssayQuiz => {
            r#"{
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 8,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string" },
                            "instructions": { "type": "string" },
                            "model_answer": { "type": "string" }
                        },
                        "required": ["question", "instructions", "model_answer"]
                    }
                }
            },
            "required": ["questions"]
        }"#
        }
        ArtifactType::CompletionQuiz => {
            r#"{
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 8,
                    "items": {
                        "type": "object",
                        "properties": {
                            "sentence": { "type": "string" },
                            "answer": { "type": "string" },
                            "hint": { "type": "string" }
                        },
                        "required": ["sentence", "answer", "hint"]
                    }
                }
            },
            "required": ["items"]
        }"#
        }
        ArtifactType::Summary => {
            r#"{
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "summary": { "type": "string" },
                "key_points": { "type": "array", "items": { "type": "string" }, "minItems": 3, "maxItems": 6 }
            },
            "required": ["title", "summary", "key_points"]
        }"#
        }
        ArtifactType::MindMap => {
            r#"{
            "type": "object",
            "properties": {
                "topic": { "type": "string" },
                "branches": {
                    "type": "array",
                    "maxItems": 8,
                    "items": {
                        "type": "object",
                        "properties": {
                            "label": { "type": "string" },
                            "children": {
                                "type": "array",
                                "maxItems": 12,
                                "items": { "type": "string" }
                            }
                        },
                        "required": ["label", "children"]
                    }
                }
            },
            "required": ["topic", "branches"]
        }"#
        }
    }
}

/// System prompt used when building the chat template for a generation call.
pub fn system_prompt_for(_artifact_type: &ArtifactType) -> &'static str {
    "You are an educational assessment generator. Base every answer strictly \
     on the provided source passages. Reply only with valid JSON matching the schema."
}

/// User-facing instructions for the requested artifact type. `context` holds the
/// retrieved source chunks.
pub fn user_message_for(artifact_type: &ArtifactType, context: &str) -> String {
    let task = match artifact_type {
        ArtifactType::MultipleChoiceQuiz => {
            "Generate between one and eight multiple-choice questions testing \
             higher-order thinking (analysis, application, or evaluation), drawn \
             from the passages. Every question must have exactly 4 plausible \
             options and the index of the correct answer."
        }
        ArtifactType::EssayQuiz => {
            "Generate between one and eight essay questions requiring students to \
             explain, compare, or evaluate concepts from the passages, each with \
             clear instructions and a model answer."
        }
        ArtifactType::CompletionQuiz => {
            "Generate between one and eight fill-in-the-blank sentences drawn from \
             the passages, each with the expected answer and a hint."
        }
        ArtifactType::Summary => {
            "Write a focused summary section for this slice of the source, \
             capturing its main ideas and key points."
        }
        ArtifactType::MindMap => {
            "Extract the topic covered by this slice of the source and its major \
             branches, each branch with a short list of child concepts."
        }
    };
    format!("{task}\n\nPassages:\n{context}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use llama_cpp_2::json_schema_to_grammar;

    #[test]
    fn test_all_schemas_compile_to_grammar() {
        for artifact_type in [
            ArtifactType::MultipleChoiceQuiz,
            ArtifactType::EssayQuiz,
            ArtifactType::CompletionQuiz,
            ArtifactType::Summary,
            ArtifactType::MindMap,
        ] {
            let schema = schema_for(&artifact_type);
            let grammar = json_schema_to_grammar(schema).unwrap_or_else(|error| {
                panic!("{artifact_type:?} schema should compile into grammar: {error}")
            });
            assert!(!grammar.is_empty());
        }
    }

    #[test]
    fn test_items_field_for() {
        assert_eq!(
            items_field_for(&ArtifactType::MultipleChoiceQuiz),
            Some("questions")
        );
        assert_eq!(items_field_for(&ArtifactType::EssayQuiz), Some("questions"));
        assert_eq!(
            items_field_for(&ArtifactType::CompletionQuiz),
            Some("items")
        );
        assert_eq!(items_field_for(&ArtifactType::Summary), None);
        assert_eq!(items_field_for(&ArtifactType::MindMap), None);
    }
}
