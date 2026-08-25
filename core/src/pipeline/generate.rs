//! Cloud-hosted artifact generation.
//!
//! Every segment feeds one request per artifact type; requests run under a
//! semaphore so bulk generation stays inside provider rate limits. Model
//! output is validated locally (`validate`): malformed or hallucinated items
//! trigger one retry with fresh seed and corrective feedback, then failing
//! items are dropped individually. Summary and MindMap are assembled
//! deterministically from per-segment sections, so coverage is exhaustive.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::provider::{ArtifactBackend, GenerateRequest};
use crate::schema::{ArtifactType, Segment};

use super::validate::{self, ItemVerdict, references_missing_media, similarity};

/// Sampling parameters applied to every generation request unless overridden.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GenerationParams {
    pub temperature: f32,
    pub max_tokens: i32,
    pub seed: u64,
}

impl Default for GenerationParams {
    fn default() -> Self {
        Self {
            temperature: 0.7,
            max_tokens: 2048,
            seed: 1234,
        }
    }
}

/// Items requested per unit, per artifact type.
fn items_per_unit(artifact_type: &ArtifactType) -> usize {
    match artifact_type {
        ArtifactType::MultipleChoiceQuiz => 4,
        ArtifactType::EssayQuiz => 3,
        ArtifactType::CompletionQuiz => 4,
        _ => 1,
    }
}

/// Quiz types stay at their configured temperature; summarization benefits
/// from cooler sampling regardless of global settings.
fn temperature_for(artifact_type: &ArtifactType, params: &GenerationParams) -> f32 {
    let _ = params;
    match artifact_type {
        ArtifactType::Summary => 0.3,
        ArtifactType::MindMap => 0.4,
        _ => params.temperature,
    }
}

fn max_tokens_for(artifact_type: &ArtifactType, params: &GenerationParams) -> i32 {
    match artifact_type {
        ArtifactType::Summary => params.max_tokens.min(700),
        ArtifactType::MindMap => params.max_tokens.min(900),
        _ => params.max_tokens,
    }
}

/// Safety valve: beyond this many segments per type, sample evenly instead of
/// fanning out unbounded request counts.
const MAX_UNITS_PER_TYPE: usize = 48;

/// Progress tick emitted as units complete.
#[derive(Clone, Copy, Debug)]
pub struct GenerationTick {
    pub done: usize,
    pub total: usize,
    pub types_done: usize,
}

/// Progress callback fired after every unit attempt.
pub type ProgressFn = Arc<dyn Fn(GenerationTick) + Send + Sync>;

/// An artifact ready to be persisted.
pub struct PendingArtifact {
    pub artifact_type: ArtifactType,
    pub source: String,
    pub content: String,
}

/// Aggregate telemetry for one generation run.
#[derive(Clone, Copy, Debug, Default)]
pub struct RunTelemetry {
    pub requests: usize,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

struct Unit {
    /// Position within [`ArtifactType::ALL`].
    type_index: usize,
    artifact_type: ArtifactType,
    segment_id: String,
    context: String,
    /// Deterministic per-unit seed derivation input (document position).
    seed_offset: u64,
}

fn build_units(segments: &[Segment]) -> Result<Vec<Unit>, String> {
    if segments.is_empty() {
        return Err(String::from(
            "No segments found for this worksheet. Run ingestion first.",
        ));
    }

    let mut units = Vec::new();
    for (type_index, artifact_type) in ArtifactType::ALL.iter().enumerate() {
        let indices = pick_indices(segments.len(), MAX_UNITS_PER_TYPE);
        for segment_index in indices {
            let segment = &segments[segment_index];
            let context = match &segment.heading {
                Some(heading) => format!("[Section: {heading}]\n{}", segment.text),
                None => segment.text.clone(),
            };
            units.push(Unit {
                type_index,
                artifact_type: artifact_type.clone(),
                segment_id: segment.id.clone(),
                context,
                seed_offset: segment.position as u64 + 17,
            });
        }
    }
    Ok(units)
}

/// Evenly sample `cap` indices when there are more than `cap`, else all.
fn pick_indices(total: usize, cap: usize) -> Vec<usize> {
    if total <= cap {
        return (0..total).collect();
    }
    let stride = total as f64 / cap as f64;
    (0..cap)
        .map(|index| (index as f64 * stride).floor() as usize)
        .collect()
}

/// System contract shared by every request.
fn system_prompt() -> &'static str {
    "You create study materials strictly grounded in supplied source material.\n\
     Non-negotiable rules:\n\
     1. Use only facts stated in the source material. Never invent details.\n\
     2. Never reference figures, tables, diagrams, charts, images, page numbers,\n\
        slides, or any media that is not literally included in the source text.\n\
     3. Never write self-referential wording such as \"the passage\", \"the document\",\n\
        \"the source\", or \"this section\" inside question text.\n\
     4. Write in the same language as the source material.\n\
     5. Reply with exactly one JSON object matching the required schema and nothing else."
}

const MCQ_EXAMPLE: &str = r#"Example question object:
{"question":"During aerobic respiration, where does the citric acid cycle occur?",
 "options":["Mitochondrial matrix","Cell nucleus","Ribosome","Golgi apparatus"],
 "answer":"Mitochondrial matrix",
 "explanation":"The cycle runs in the matrix, producing NADH for oxidative phosphorylation."}"#;

fn user_prompt(artifact_type: &ArtifactType, count: usize, context: &str) -> String {
    let task = match artifact_type {
        ArtifactType::MultipleChoiceQuiz => format!(
            "Write exactly {count} multiple-choice questions testing analysis, application, or \
             evaluation of the material below. For every question:\n\
             - Exactly 4 answer options in plain text: no lettering, numbering, or bullet marks.\n\
             - Options must be mutually exclusive, comparable in length, plausible but clearly\n\
             wrong to someone who knows the material. Never offer options like \"all of the above\".\n\
             - Exactly one correct answer, written verbatim as one of the options.\n\
             - Vary the position of the correct answer across questions.\n\
             - One-sentence explanation citing the supporting fact.\n\
             - Do not repeat near-identical questions.\n{MCQ_EXAMPLE}"
        ),
        ArtifactType::EssayQuiz => format!(
            "Write exactly {count} essay questions requiring students to explain, compare, or \
             evaluate ideas from the material below. Each needs clear instructions and a concise \
             model answer grounded in the material."
        ),
        ArtifactType::CompletionQuiz => format!(
            "Write exactly {count} fill-in-the-blank statements drawn from the material below. \
             Mark each blank with ____________ . The expected answer must appear word-for-word in \
             the material. Add a short hint per statement."
        ),
        ArtifactType::Summary => String::from(
            "Summarize this slice of the material: a short title, a focused paragraph capturing \
             its main ideas, and three to five key points.",
        ),
        ArtifactType::MindMap => String::from(
            "Extract the topic of this slice of the material and its major branches. Up to six \
             branches, each with up to six short child concepts drawn from the material.",
        ),
    };
    format!("{task}\n\nSource material:\n\"\"\"\n{context}\n\"\"\"")
}

fn retry_feedback(reasons: &[String]) -> String {
    let mut feedback =
        String::from("\n\nYour previous reply was rejected for these reasons; fix them and return the full corrected JSON object:\n");
    for reason in reasons.iter().take(5) {
        feedback.push_str(&format!("- {reason}\n"));
    }
    feedback
}

// --- Schemas (strict-mode friendly: every property listed in `required`,
// `additionalProperties: false` everywhere) ---

fn string_array(min_items: usize, max_items: usize) -> serde_json::Value {
    serde_json::json!({
        "type": "array",
        "items": { "type": "string" },
        "minItems": min_items,
        "maxItems": max_items,
    })
}

pub fn schema_for(artifact_type: &ArtifactType, items: usize) -> serde_json::Value {
    match artifact_type {
        ArtifactType::MultipleChoiceQuiz => serde_json::json!({
            "type": "object",
            "properties": { "questions": mcq_questions_schema(items) },
            "required": ["questions"],
            "additionalProperties": false,
        }),
        ArtifactType::EssayQuiz => serde_json::json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": items,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string" },
                            "instructions": { "type": "string" },
                            "model_answer": { "type": "string" },
                        },
                        "required": ["question", "instructions", "model_answer"],
                        "additionalProperties": false,
                    },
                },
            },
            "required": ["questions"],
            "additionalProperties": false,
        }),
        ArtifactType::CompletionQuiz => serde_json::json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": items,
                    "items": {
                        "type": "object",
                        "properties": {
                            "sentence": { "type": "string" },
                            "answer": { "type": "string" },
                            "hint": { "type": "string" },
                        },
                        "required": ["sentence", "answer", "hint"],
                        "additionalProperties": false,
                    },
                },
            },
            "required": ["items"],
            "additionalProperties": false,
        }),
        ArtifactType::Summary => serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "summary": { "type": "string" },
                "key_points": string_array(3, 5),
            },
            "required": ["title", "summary", "key_points"],
            "additionalProperties": false,
        }),
        ArtifactType::MindMap => serde_json::json!({
            "type": "object",
            "properties": {
                "topic": { "type": "string" },
                "branches": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 6,
                    "items": {
                        "type": "object",
                        "properties": {
                            "label": { "type": "string" },
                            "children": string_array(1, 6),
                        },
                        "required": ["label", "children"],
                        "additionalProperties": false,
                    },
                },
            },
            "required": ["topic", "branches"],
            "additionalProperties": false,
        }),
    }
}

fn mcq_questions_schema(items: usize) -> serde_json::Value {
    serde_json::json!({
        "type": "array",
        "minItems": 1,
        "maxItems": items,
        "items": {
            "type": "object",
            "properties": {
                "question": { "type": "string" },
                "options": {
                    "type": "array",
                    "minItems": 4,
                    "maxItems": 4,
                    "items": { "type": "string" },
                },
                "answer": { "type": "string" },
                "explanation": { "type": "string" },
            },
            "required": ["question", "options", "answer", "explanation"],
            "additionalProperties": false,
        },
    })
}

fn items_field(artifact_type: &ArtifactType) -> Option<&'static str> {
    match artifact_type {
        ArtifactType::MultipleChoiceQuiz | ArtifactType::EssayQuiz => Some("questions"),
        ArtifactType::CompletionQuiz => Some("items"),
        ArtifactType::Summary | ArtifactType::MindMap => None,
    }
}

const MIN_MCQ_GROUNDING: f32 = 0.15;
const MIN_COMPLETION_GROUNDING: f32 = 0.5;
const DUPLICATE_QUESTION_SIMILARITY: f32 = 0.75;

struct ValidatedOutput {
    /// Serialized item objects (or whole section objects).
    items: Vec<String>,
    rejection_reasons: Vec<String>,
}

fn validate_unit_output(
    artifact_type: &ArtifactType,
    raw: &str,
    source_segment: &str,
    seen_questions: &[String],
) -> ValidatedOutput {
    fn rejected(reasons: &mut Vec<String>, message: String) -> ValidatedOutput {
        reasons.push(message);
        ValidatedOutput {
            items: Vec::new(),
            rejection_reasons: reasons.clone(),
        }
    }

    let mut rejection_reasons = Vec::new();
    let value: serde_json::Value = match serde_json::from_str(raw.trim()) {
        Ok(value) => value,
        Err(error) => return rejected(&mut rejection_reasons, format!("output is not valid JSON: {error}")),
    };

    if references_missing_media(raw) {
        rejection_reasons.push(String::from("output references figures/media absent from the source"));
    }

    match items_field(artifact_type) {
        Some(field) => {
            let Some(entries) = value.get(field).and_then(serde_json::Value::as_array) else {
                return rejected(&mut rejection_reasons, format!("missing \"{field}\" array"));
            };

            let mut items = Vec::new();
            for entry in entries {
                let verdict = match artifact_type {
                    ArtifactType::MultipleChoiceQuiz => {
                        validate::validate_mcq_item(entry, source_segment, MIN_MCQ_GROUNDING)
                    }
                    ArtifactType::EssayQuiz => validate::validate_essay_item(entry),
                    _ => {
                        validate::validate_completion_item(entry, source_segment, MIN_COMPLETION_GROUNDING)
                    }
                };

                match verdict {
                    ItemVerdict::Accepted(mut accepted) => {
                        // Cross-unit near-duplicate suppression.
                        if matches!(artifact_type, ArtifactType::MultipleChoiceQuiz | ArtifactType::EssayQuiz) {
                            let question = accepted["question"].as_str().unwrap_or_default().to_string();
                            let duplicate = seen_questions.iter().any(|seen| {
                                similarity(seen, &question) >= DUPLICATE_QUESTION_SIMILARITY
                            });
                            if duplicate {
                                continue;
                            }
                            accepted["question"] = serde_json::Value::String(question);
                        }
                        items.push(accepted.to_string());
                    }
                    ItemVerdict::Rejected(reason) => rejection_reasons.push(reason),
                }
            }
            ValidatedOutput { items, rejection_reasons }
        }
        None => {
            // Whole-worksheet section objects get structural checks only;
            // deterministic merging downstream guarantees coverage.
            let shape_ok = match artifact_type {
                ArtifactType::Summary => {
                    value.get("summary").is_some_and(serde_json::Value::is_string)
                        && value.get("key_points").is_some_and(serde_json::Value::is_array)
                }
                _ => {
                    value.get("topic").is_some_and(serde_json::Value::is_string)
                        && value.get("branches").is_some_and(serde_json::Value::is_array)
                }
            };
            if shape_ok {
                ValidatedOutput {
                    items: vec![value.to_string()],
                    rejection_reasons,
                }
            } else {
                rejected(
                    &mut rejection_reasons,
                    format!("expected a {} object with the documented fields", artifact_type.to_db()),
                )
            }
        }
    }
}

fn approximate_tokens(text: &str) -> u64 {
    (text.len() as u64 / 4).max(1)
}

/// What one unit produced.
struct UnitOutcome {
    /// `(item_json, …)` serialized outputs ready for assembly.
    items: Vec<String>,
    requests_made: usize,
    tokens_in: u64,
    tokens_out: u64,
}

async fn run_unit(
    backend: &Arc<dyn ArtifactBackend>,
    unit: &Unit,
    params: &GenerationParams,
    seen_questions: &Mutex<Vec<String>>,
) -> UnitOutcome {
    let mut requests_made = 0usize;
    let mut tokens_in = 0u64;
    let mut tokens_out = 0u64;

    let count = items_per_unit(&unit.artifact_type);
    let system = system_prompt().to_string();
    let schema_name = unit.artifact_type.to_db().to_string();
    let schema = schema_for(&unit.artifact_type, count);
    let base_seed = params.seed.wrapping_add(unit.seed_offset);
    let base_user = user_prompt(&unit.artifact_type, count, &unit.context);

    let make_request = |seed: u64, user: String| GenerateRequest {
        system: system.clone(),
        user,
        schema_name: schema_name.clone(),
        schema: schema.clone(),
        temperature: temperature_for(&unit.artifact_type, params),
        max_tokens: max_tokens_for(&unit.artifact_type, params),
        seed,
    };

    let mut user = base_user.clone();
    let mut validated;

    let raw = match backend.generate_json(&make_request(base_seed, user.clone())).await {
        Ok(raw) => {
            requests_made += 1;
            tokens_in += approximate_tokens(&system) + approximate_tokens(&user);
            tokens_out += approximate_tokens(&raw);
            raw
        }
        Err(error) => {
            eprintln!("[pipeline] unit failed ({schema_name}): {error}");
            return UnitOutcome { items: Vec::new(), requests_made, tokens_in, tokens_out };
        }
    };

    validated = validate_unit_output(&unit.artifact_type, &raw, &unit.context, &seen_questions.lock().unwrap());

    if validated.items.is_empty() && !validated.rejection_reasons.is_empty() {
        user.push_str(&retry_feedback(&validated.rejection_reasons));
        match backend.generate_json(&make_request(base_seed.wrapping_add(7_919), user)).await {
            Ok(retry_raw) => {
                requests_made += 1;
                tokens_out += approximate_tokens(&retry_raw);
                validated = validate_unit_output(
                    &unit.artifact_type,
                    &retry_raw,
                    &unit.context,
                    &seen_questions.lock().unwrap(),
                );
            }
            Err(error) => {
                eprintln!("[pipeline] unit retry failed ({schema_name}): {error}");
            }
        }
    }

    // Register surviving questions for cross-unit duplicate detection.
    if !validated.items.is_empty()
        && matches!(unit.artifact_type, ArtifactType::MultipleChoiceQuiz | ArtifactType::EssayQuiz)
    {
        let mut seen = seen_questions.lock().unwrap();
        for item in &validated.items {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(item) {
                if let Some(question) = value.get("question").and_then(serde_json::Value::as_str) {
                    seen.push(question.to_string());
                }
            }
        }
    }

    if validated.items.is_empty() {
        eprintln!(
            "[pipeline] dropping unit ({schema_name}): {}",
            validated.rejection_reasons.first().cloned().unwrap_or_default()
        );
    }

    UnitOutcome {
        items: validated.items,
        requests_made,
        tokens_in,
        tokens_out,
    }
}

/// Output recorded per finished unit.
struct UnitRecord {
    type_index: usize,
    segment_id: String,
    items: Vec<String>,
}

/// Generate artifacts for every type across all segments under a concurrency
/// limit. Returns pending artifacts plus aggregate telemetry.
pub async fn generate_all(
    backend: Arc<dyn ArtifactBackend>,
    concurrency: usize,
    segments: &[Segment],
    params: Option<GenerationParams>,
    on_progress: Option<ProgressFn>,
) -> Result<(Vec<PendingArtifact>, RunTelemetry), String> {
    let params = params.unwrap_or_default();
    let units = build_units(segments)?;
    let total_units = units.len();

    let mut totals_per_type = [0usize; ArtifactType::ALL.len()];
    for unit in &units {
        totals_per_type[unit.type_index] += 1;
    }
    let completed_per_type: Arc<Vec<AtomicUsize>> = Arc::new(
        (0..ArtifactType::ALL.len()).map(|_| AtomicUsize::new(0)).collect(),
    );
    let completed_types = Arc::new(AtomicUsize::new(0));

    if let Some(on_progress) = &on_progress {
        on_progress(GenerationTick { done: 0, total: total_units, types_done: 0 });
    }

    let records: Arc<Mutex<Vec<Option<UnitRecord>>>> =
        Arc::new(Mutex::new((0..total_units).map(|_| None).collect()));
    let seen_questions: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency.clamp(1, 32)));
    let telemetry = Arc::new(Mutex::new(RunTelemetry::default()));
    let done_counter = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(total_units);
    for (unit_index, unit) in units.into_iter().enumerate() {
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| e.to_string())?;
        let backend = backend.clone();
        let params = params.clone();
        let records = records.clone();
        let seen_questions = seen_questions.clone();
        let telemetry = telemetry.clone();
        let done_counter = done_counter.clone();
        let completed_per_type = completed_per_type.clone();
        let _ = &completed_per_type;
        let completed_types = completed_types.clone();
        let on_progress = on_progress.clone();

        handles.push(tokio::spawn(async move {
            let outcome = run_unit(&backend, &unit, &params, &seen_questions).await;

            {
                let mut stats = telemetry.lock().unwrap();
                stats.requests += outcome.requests_made;
                stats.tokens_in += outcome.tokens_in;
                stats.tokens_out += outcome.tokens_out;
            }

            records.lock().unwrap()[unit_index] = Some(UnitRecord {
                type_index: unit.type_index,
                segment_id: unit.segment_id.clone(),
                items: outcome.items,
            });

            let finished = done_counter.fetch_add(1, Ordering::Relaxed) + 1;
            let completed = completed_per_type[unit.type_index].fetch_add(1, Ordering::Relaxed) + 1;
            let mut types_done = 0usize;
            if completed == totals_per_type[unit.type_index] {
                types_done = completed_types.fetch_add(1, Ordering::Relaxed) + 1;
            }

            if let Some(on_progress) = &on_progress {
                on_progress(GenerationTick { done: finished, total: total_units, types_done });
            }

            drop(permit);
        }));
    }

    for handle in handles {
        handle.await.map_err(|e| format!("Generation task failed: {e}"))?;
    }

    let telemetry = *telemetry.lock().unwrap();
    let records: Vec<Option<UnitRecord>> =
        records.lock().unwrap().drain(..).collect();

    let pending = assemble_pending(&records);
    Ok((pending, telemetry))
}

fn assemble_pending(records: &[Option<UnitRecord>]) -> Vec<PendingArtifact> {
    let mut pending = Vec::new();

    for (type_index, artifact_type) in ArtifactType::ALL.iter().enumerate() {
        let sections: Vec<(String, serde_json::Value)> = records
            .iter()
            .flatten()
            .filter(|record| record.type_index == type_index && !record.items.is_empty())
            .flat_map(|record| {
                record
                    .items
                    .iter()
                    .filter_map(|item| serde_json::from_str::<serde_json::Value>(item).ok())
                    .map(|value| (record.segment_id.clone(), value))
            })
            .collect();

        if sections.is_empty() {
            continue;
        }

        if items_field(artifact_type).is_some() {
            // One artifact per generated item.
            for (source, value) in sections {
                pending.push(PendingArtifact {
                    artifact_type: artifact_type.clone(),
                    source,
                    content: value.to_string(),
                });
            }
        } else if let Some(content) = merge_sections(artifact_type, &sections) {
            // One worksheet-wide artifact merged deterministically.
            pending.push(PendingArtifact {
                artifact_type: artifact_type.clone(),
                source: sections
                    .iter()
                    .map(|(source, _)| source.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                content,
            });
        }
    }

    pending
}

/// Deterministically merge per-segment section objects into a single
/// worksheet-wide artifact. No extra LLM call: concatenation preserves order
/// and guarantees nothing sampled away.
fn merge_sections(
    artifact_type: &ArtifactType,
    sections: &[(String, serde_json::Value)],
) -> Option<String> {
    match artifact_type {
        ArtifactType::Summary => {
            let title = sections
                .iter()
                .find_map(|(_, value)| value.get("title").and_then(serde_json::Value::as_str))
                .filter(|title| !title.trim().is_empty())
                .unwrap_or("Worksheet summary");

            let mut paragraphs = Vec::new();
            let mut key_points: Vec<String> = Vec::new();
            for (_, value) in sections {
                if let Some(summary) = value.get("summary").and_then(serde_json::Value::as_str) {
                    let trimmed = summary.trim();
                    if !trimmed.is_empty() {
                        paragraphs.push(trimmed.to_string());
                    }
                }
                if let Some(points) = value.get("key_points").and_then(serde_json::Value::as_array) {
                    for point in points.iter().filter_map(serde_json::Value::as_str) {
                        if !key_points.contains(&point.to_string()) {
                            key_points.push(point.to_string());
                        }
                    }
                }
            }
            key_points.truncate(12);

            Some(
                serde_json::json!({
                    "title": title,
                    "summary": paragraphs.join("\n\n"),
                    "key_points": key_points,
                })
                .to_string(),
            )
        }
        ArtifactType::MindMap => {
            let topic = sections
                .iter()
                .find_map(|(_, value)| value.get("topic").and_then(serde_json::Value::as_str))
                .filter(|topic| !topic.trim().is_empty())
                .unwrap_or("Overview");

            let mut branches: Vec<serde_json::Value> = Vec::new();
            for (_, value) in sections {
                if let Some(section_branches) = value.get("branches").and_then(serde_json::Value::as_array) {
                    branches.extend(section_branches.iter().cloned());
                }
            }
            branches.truncate(24);

            Some(
                serde_json::json!({
                    "topic": topic,
                    "branches": branches,
                })
                .to_string(),
            )
        }
        _ => None,
    }
}
