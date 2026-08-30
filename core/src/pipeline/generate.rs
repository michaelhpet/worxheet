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

use crate::logging::{self, RunLogs};
use crate::provider::{ArtifactBackend, GenerateRequest};
use crate::schema::{ArtifactType, Segment};

use super::validate::{self, references_missing_media, similarity, ItemVerdict};

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

/// Progress callback fires after every unit attempt.
pub type ProgressFn = Arc<dyn Fn(GenerationTick) + Send + Sync>;

/// Persistence hook. When provided, artifacts are routed here as each unit
/// completes (incremental, so interruptions keep finished parts); when `None`,
/// all artifacts are returned as `pending` for the caller to persist.
pub type PersistFn = Arc<dyn Fn(Vec<PendingArtifact>) + Send + Sync>;

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
    /// Number of units that failed to reach the provider (transport/LLM
    /// errors), as opposed to being dropped at validation time.
    pub backend_errors: usize,
}

/// Aggregates which artifacts already exist for a worksheet, so a resumed run
/// can skip already-generated parts. Per-item quiz units are keyed by
/// `(artifact_type, source)` where `source` is the originating segment id;
/// merged types (Summary/MindMap) are treated as atomic per artifact type.
#[derive(Clone, Debug, Default)]
pub struct ExistingArtifacts {
    /// Persisted per-item units: `(artifact_type db string, source)`.
    pub done_items: std::collections::HashSet<(String, String)>,
    /// Persisted merged artifact types: `artifact_type` db string.
    pub done_merged: std::collections::HashSet<String>,
}

impl ExistingArtifacts {
    fn unit_done(&self, artifact_type: &ArtifactType, segment_id: &str) -> bool {
        match items_field(artifact_type) {
            Some(_) => self
                .done_items
                .contains(&(artifact_type.to_db().to_string(), segment_id.to_string())),
            None => self.done_merged.contains(artifact_type.to_db()),
        }
    }
}

struct Unit {
    /// Position within [`ArtifactType::ALL`].
    type_index: usize,
    artifact_type: ArtifactType,
    segment_id: String,
    context: String,
    /// Deterministic per-unit seed derivation input (document position).
    seed_offset: u64,
    /// Whether this unit's artifact already exists and should be skipped.
    skip: bool,
}

fn build_units(segments: &[Segment], existing: &ExistingArtifacts) -> Result<Vec<Unit>, String> {
    if segments.is_empty() {
        return Err(String::from(
            "No segments found for this worksheet. Run ingestion first.",
        ));
    }

    // Guard generation against content-less units. Segmentation guarantees
    // real segments for freshly ingested files, but stale or reused chunks can
    // still carry empty slivers — page/number markers, dot leaders, boilerplate
    // — whose near-empty context makes the model reply with nothing, which
    // fails validation. Sampling from the remaining ordered pool keeps coverage
    // while never emitting an empty context.
    let usable: Vec<&Segment> = segments
        .iter()
        .filter(|segment| !is_degenerate_segment(segment))
        .collect();
    if usable.is_empty() {
        return Err(String::from(
            "No usable segments found for this worksheet. Re-ingest the source files.",
        ));
    }

    let mut units = Vec::new();
    for (type_index, artifact_type) in ArtifactType::ALL.iter().enumerate() {
        let indices = pick_indices(usable.len(), MAX_UNITS_PER_TYPE);
        for segment_index in indices {
            let segment = usable[segment_index];
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
                skip: existing.unit_done(artifact_type, &segment.id),
            });
        }
    }
    Ok(units)
}

/// True when a segment carries no usable source material and must never be
/// sent to the model. Deliberately content-based and format-agnostic: it does
/// not assume any particular marker, numbering scheme, or material shape.
/// Leaves that are indistinguishable from noise — empty text, page/number
/// markers, dot leaders, numeral runs, spot boilerplate — share one trait:
/// almost none of their tokens are real words. Rejecting those keeps
/// legitimately short but prose-bearing segments.
fn is_degenerate_segment(segment: &Segment) -> bool {
    fn meaningful_words(text: &str) -> usize {
        text.split_whitespace()
            .filter(|word| word.chars().any(char::is_alphabetic))
            .count()
    }

    let text = segment.text.trim();
    text.is_empty() || meaningful_words(text) < MIN_CONTENT_WORDS
}

/// A unit's source context needs enough real prose to ground an answer; a
/// couple of full sentences is the smallest plausible source.
const MIN_CONTENT_WORDS: usize = 10;

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
        Err(error) => {
            return rejected(
                &mut rejection_reasons,
                format!("output is not valid JSON: {error}"),
            )
        }
    };

    if references_missing_media(raw) {
        rejection_reasons.push(String::from(
            "output references figures/media absent from the source",
        ));
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
                    _ => validate::validate_completion_item(
                        entry,
                        source_segment,
                        MIN_COMPLETION_GROUNDING,
                    ),
                };

                match verdict {
                    ItemVerdict::Accepted(mut accepted) => {
                        // Cross-unit near-duplicate suppression.
                        if matches!(
                            artifact_type,
                            ArtifactType::MultipleChoiceQuiz | ArtifactType::EssayQuiz
                        ) {
                            let question = accepted["question"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string();
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
            ValidatedOutput {
                items,
                rejection_reasons,
            }
        }
        None => {
            // Whole-worksheet section objects get structural checks only;
            // deterministic merging downstream guarantees coverage.
            let shape_ok = match artifact_type {
                ArtifactType::Summary => {
                    value
                        .get("summary")
                        .is_some_and(serde_json::Value::is_string)
                        && value
                            .get("key_points")
                            .is_some_and(serde_json::Value::is_array)
                }
                _ => {
                    value.get("topic").is_some_and(serde_json::Value::is_string)
                        && value
                            .get("branches")
                            .is_some_and(serde_json::Value::is_array)
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
                    format!(
                        "expected a {} object with the documented fields",
                        artifact_type.to_db()
                    ),
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
    /// Set when the request could not reach / be completed by the provider
    /// (connection, auth, rate-limit, etc.) rather than being a content-level
    /// rejection. Used to distinguish a provider outage from dropped items.
    backend_error: Option<String>,
}

async fn run_unit(
    backend: &Arc<dyn ArtifactBackend>,
    unit: &Unit,
    params: &GenerationParams,
    seen_questions: &Mutex<Vec<String>>,
    logs: Option<&RunLogs>,
) -> UnitOutcome {
    let mut requests_made = 0usize;
    let mut tokens_in = 0u64;
    let mut tokens_out = 0u64;
    let mut backend_error: Option<String> = None;

    let count = items_per_unit(&unit.artifact_type);
    let system = system_prompt().to_string();
    let schema_name = unit.artifact_type.to_db().to_string();
    let schema = schema_for(&unit.artifact_type, count);
    let base_seed = params.seed.wrapping_add(unit.seed_offset);
    let base_user = user_prompt(&unit.artifact_type, count, &unit.context);

    let unit_type = unit.artifact_type.to_db();
    let schema_label = logging::sanitize_label(&schema_name);

    let make_request = |seed: u64, user: String| GenerateRequest {
        system: system.clone(),
        user,
        schema_name: schema_name.clone(),
        schema: schema.clone(),
        temperature: temperature_for(&unit.artifact_type, params),
        max_tokens: max_tokens_for(&unit.artifact_type, params),
        seed,
    };

    // Best-effort generation trace: one request artifact per attempt, one
    // response artifact per call, all under logs/generation/.
    let log_request = |attempt: usize, request: &GenerateRequest| {
        if let Some(logs) = logs {
            let record = serde_json::json!({
                "timestamp": logging::rfc3339_utc(),
                "unit_type": unit_type,
                "segment_id": unit.segment_id,
                "attempt": attempt,
                "seed": request.seed,
                "system": request.system,
                "user": request.user,
                "schema_name": request.schema_name,
                "schema": request.schema,
                "temperature": request.temperature,
                "max_tokens": request.max_tokens,
            });
            let label = format!("{}_seed{}_attempt{}", schema_label, request.seed, attempt);
            logs.write_json("generation", &label, &record);
        }
    };

    let log_response = |attempt: usize, seed: u64, record: &serde_json::Value| {
        if let Some(logs) = logs {
            let label = format!("{}_seed{}_attempt{}_response", schema_label, seed, attempt);
            logs.write_json("generation", &label, record);
        }
    };

    let mut user = base_user.clone();

    // First attempt.
    let request = make_request(base_seed, user.clone());
    log_request(0, &request);
    let started = std::time::Instant::now();
    let raw = match backend.generate_json(&request).await {
        Ok(raw) => {
            requests_made += 1;
            tokens_in += approximate_tokens(&system) + approximate_tokens(&user);
            tokens_out += approximate_tokens(&raw);
            raw
        }
        Err(error) => {
            log_response(
                0,
                base_seed,
                &serde_json::json!({
                    "timestamp": logging::rfc3339_utc(),
                    "unit_type": unit_type,
                    "segment_id": unit.segment_id,
                    "attempt": 0,
                    "elapsed_ms": started.elapsed().as_millis(),
                    "status": "error",
                    "error": format!("{error}"),
                }),
            );
            eprintln!(
                "[{}] [pipeline] unit failed ({schema_name}): {error}",
                logging::rfc3339_utc()
            );
            return UnitOutcome {
                items: Vec::new(),
                requests_made,
                tokens_in,
                tokens_out,
                backend_error: Some(format!("{error}")),
            };
        }
    };

    let mut validated = validate_unit_output(
        &unit.artifact_type,
        &raw,
        &unit.context,
        &seen_questions.lock().unwrap(),
    );
    log_response(
        0,
        base_seed,
        &serde_json::json!({
            "timestamp": logging::rfc3339_utc(),
            "unit_type": unit_type,
            "segment_id": unit.segment_id,
            "attempt": 0,
            "elapsed_ms": started.elapsed().as_millis(),
            "status": "ok",
            "verdict": if validated.items.is_empty() { "rejected" } else { "ok" },
            "raw": raw,
            "rejection_reasons": validated.rejection_reasons.clone(),
        }),
    );

    if validated.items.is_empty() && !validated.rejection_reasons.is_empty() {
        user.push_str(&retry_feedback(&validated.rejection_reasons));
        let request = make_request(base_seed.wrapping_add(7_919), user);
        log_request(1, &request);
        let started = std::time::Instant::now();
        match backend.generate_json(&request).await {
            Ok(retry_raw) => {
                requests_made += 1;
                tokens_out += approximate_tokens(&retry_raw);
                validated = validate_unit_output(
                    &unit.artifact_type,
                    &retry_raw,
                    &unit.context,
                    &seen_questions.lock().unwrap(),
                );
                log_response(
                    1,
                    request.seed,
                    &serde_json::json!({
                        "timestamp": logging::rfc3339_utc(),
                        "unit_type": unit_type,
                        "segment_id": unit.segment_id,
                        "attempt": 1,
                        "elapsed_ms": started.elapsed().as_millis(),
                        "status": "ok",
                        "verdict": if validated.items.is_empty() { "rejected" } else { "ok" },
                        "raw": retry_raw,
                        "rejection_reasons": validated.rejection_reasons.clone(),
                    }),
                );
            }
            Err(error) => {
                log_response(
                    1,
                    request.seed,
                    &serde_json::json!({
                        "timestamp": logging::rfc3339_utc(),
                        "unit_type": unit_type,
                        "segment_id": unit.segment_id,
                        "attempt": 1,
                        "elapsed_ms": started.elapsed().as_millis(),
                        "status": "error",
                        "error": format!("{error}"),
                    }),
                );
                eprintln!(
                    "[{}] [pipeline] unit retry failed ({schema_name}): {error}",
                    logging::rfc3339_utc()
                );
                if backend_error.is_none() {
                    backend_error = Some(format!("{error}"));
                }
            }
        }
    }

    // Register surviving questions for cross-unit duplicate detection.
    if !validated.items.is_empty()
        && matches!(
            unit.artifact_type,
            ArtifactType::MultipleChoiceQuiz | ArtifactType::EssayQuiz
        )
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
            "[{}] [pipeline] dropping unit ({schema_name}): {}",
            logging::rfc3339_utc(),
            validated
                .rejection_reasons
                .first()
                .cloned()
                .unwrap_or_default()
        );
    }

    UnitOutcome {
        items: validated.items,
        requests_made,
        tokens_in,
        tokens_out,
        backend_error,
    }
}

/// Output recorded per finished unit.
struct UnitRecord {
    type_index: usize,
    segment_id: String,
    items: Vec<String>,
}

/// Generate artifacts for every type across all segments under a concurrency
/// limit. Already-completed units (per `existing`) are skipped and not counted
/// in progress or telemetry. When `on_persist` is provided, artifacts are
/// routed to it as each unit completes (incremental persistence) and nothing
/// is returned; otherwise all artifacts are returned for the caller to persist.
/// Returns pending artifacts plus aggregate telemetry.
#[allow(clippy::too_many_arguments)]
pub async fn generate_all(
    backend: Arc<dyn ArtifactBackend>,
    concurrency: usize,
    segments: &[Segment],
    existing: ExistingArtifacts,
    params: Option<GenerationParams>,
    on_persist: Option<PersistFn>,
    on_progress: Option<ProgressFn>,
    logs: Option<Arc<RunLogs>>,
) -> Result<(Vec<PendingArtifact>, RunTelemetry), String> {
    let params = params.unwrap_or_default();
    let units = build_units(segments, &existing)?;
    let active_units: Vec<Unit> = units.into_iter().filter(|unit| !unit.skip).collect();
    let total_units = active_units.len();

    let mut totals_per_type = [0usize; ArtifactType::ALL.len()];
    for unit in &active_units {
        totals_per_type[unit.type_index] += 1;
    }
    let completed_per_type: Arc<Vec<AtomicUsize>> = Arc::new(
        (0..ArtifactType::ALL.len())
            .map(|_| AtomicUsize::new(0))
            .collect(),
    );
    let completed_types = Arc::new(AtomicUsize::new(0));

    if let Some(on_progress) = &on_progress {
        on_progress(GenerationTick {
            done: 0,
            total: total_units,
            types_done: 0,
        });
    }

    let records: Arc<Mutex<Vec<Option<UnitRecord>>>> =
        Arc::new(Mutex::new((0..total_units).map(|_| None).collect()));
    let seen_questions: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency.clamp(1, 32)));
    let telemetry = Arc::new(Mutex::new(RunTelemetry::default()));
    let first_backend_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let done_counter = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(total_units);
    for (unit_index, unit) in active_units.into_iter().enumerate() {
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
        let first_backend_error = first_backend_error.clone();
        let done_counter = done_counter.clone();
        let completed_per_type = completed_per_type.clone();
        let completed_types = completed_types.clone();
        let on_progress = on_progress.clone();
        let on_persist = on_persist.clone();
        let logs = logs.clone();

        handles.push(tokio::spawn(async move {
            let outcome =
                run_unit(&backend, &unit, &params, &seen_questions, logs.as_deref()).await;

            {
                let mut stats = telemetry.lock().unwrap();
                stats.requests += outcome.requests_made;
                stats.tokens_in += outcome.tokens_in;
                stats.tokens_out += outcome.tokens_out;
                if outcome.backend_error.is_some() {
                    stats.backend_errors += 1;
                    if let Some(message) = &outcome.backend_error {
                        let mut holder = first_backend_error.lock().unwrap();
                        if holder.is_none() {
                            *holder = Some(message.clone());
                        }
                    }
                }
            }

            let is_item_type = items_field(&unit.artifact_type).is_some();
            if is_item_type {
                // Per-item units are persisted immediately so a mid-run kill
                // keeps the finished quiz items.
                let artifacts = build_item_artifacts(&unit, &outcome.items);
                if let Some(on_persist) = &on_persist {
                    if !artifacts.is_empty() {
                        on_persist(artifacts);
                    }
                } else {
                    records.lock().unwrap()[unit_index] = Some(UnitRecord {
                        type_index: unit.type_index,
                        segment_id: unit.segment_id.clone(),
                        items: outcome.items,
                    });
                }
            } else {
                // Merged types accumulate per-segment sections and are
                // assembled once the type completes below.
                records.lock().unwrap()[unit_index] = Some(UnitRecord {
                    type_index: unit.type_index,
                    segment_id: unit.segment_id.clone(),
                    items: outcome.items,
                });
            }

            let finished = done_counter.fetch_add(1, Ordering::Relaxed) + 1;
            let completed = completed_per_type[unit.type_index].fetch_add(1, Ordering::Relaxed) + 1;
            let mut types_done = 0usize;
            if completed == totals_per_type[unit.type_index] {
                types_done = completed_types.fetch_add(1, Ordering::Relaxed) + 1;
            }

            if let Some(on_progress) = &on_progress {
                on_progress(GenerationTick {
                    done: finished,
                    total: total_units,
                    types_done,
                });
            }

            drop(permit);
        }));
    }

    for handle in handles {
        handle
            .await
            .map_err(|e| format!("Generation task failed: {e}"))?;
    }

    let telemetry = *telemetry.lock().unwrap();
    let records: Vec<Option<UnitRecord>> = records.lock().unwrap().drain(..).collect();

    // Streamed persistence leaves only merged artifacts to be returned; in the
    // non-streaming (test/legacy) path, per-item artifacts come from records.
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
            if on_persist.is_none() {
                for (source, value) in sections {
                    pending.push(PendingArtifact {
                        artifact_type: artifact_type.clone(),
                        source,
                        content: value.to_string(),
                    });
                }
            }
            continue;
        }
        if let Some(content) = merge_sections(artifact_type, &sections) {
            let artifact = PendingArtifact {
                artifact_type: artifact_type.clone(),
                source: sections
                    .iter()
                    .map(|(source, _)| source.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                content,
            };
            if let Some(on_persist) = &on_persist {
                on_persist(vec![artifact]);
            } else {
                pending.push(artifact);
            }
        }
    }

    // A provider outage (connection/auth/rate-limit) that hits every unit is a
    // hard failure, not an empty success: surface it so the worksheet is marked
    // `failed` rather than `done` with zero artifacts.
    if telemetry.backend_errors > 0 && telemetry.backend_errors == total_units {
        let first = first_backend_error
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| String::from("unknown provider error"));
        return Err(format!(
            "LLM generation failed: all {} generation request(s) could not reach \
             the provider ({first}). Check that a provider is configured and reachable.",
            telemetry.backend_errors
        ));
    }

    Ok((pending, telemetry))
}

/// One artifact per generated item for per-item quiz types, keyed by unit.
fn build_item_artifacts(unit: &Unit, items: &[String]) -> Vec<PendingArtifact> {
    items
        .iter()
        .filter_map(|item| serde_json::from_str::<serde_json::Value>(item).ok())
        .map(|value| PendingArtifact {
            artifact_type: unit.artifact_type.clone(),
            source: unit.segment_id.clone(),
            content: value.to_string(),
        })
        .collect()
}

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
                if let Some(points) = value
                    .get("key_points")
                    .and_then(serde_json::Value::as_array)
                {
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
                if let Some(section_branches) =
                    value.get("branches").and_then(serde_json::Value::as_array)
                {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(position: i32, heading: Option<&str>, text: &str) -> Segment {
        Segment {
            id: format!("seg-{position}"),
            worksheet_id: String::from("ws"),
            file_id: String::from("file"),
            position,
            heading: heading.map(str::to_string),
            text: text.to_string(),
        }
    }

    #[test]
    fn test_is_degenerate_segment_rejects_noise_not_short_prose() {
        // Reason a robust, format-agnostic check matters: none of these
        // reproduce a specific marker string, yet all are content-less noise.
        for noise in [
            "[Page 2]",
            "[Slide 3]",
            "1 2 3 4",
            ". . . . . . . . . . 182 6.4 ATP . . . . . . . . .",
            "  \t  ",
        ] {
            assert!(
                is_degenerate_segment(&segment(0, None, noise)),
                "expected {noise:?} to be degenerate"
            );
        }

        // Legitimately short real prose must survive.
        let short = "The mitochondrion is the powerhouse of the cell and respiration produces ATP.";
        assert!(!is_degenerate_segment(&segment(0, None, short)));
        let long = "OpenStax provides free, peer-reviewed, openly licensed textbooks. \
                    Every volume is written by subject experts and reviewed for accuracy.";
        assert!(!is_degenerate_segment(&segment(1, Some("COLLEGE"), long)));
    }

    #[test]
    fn test_build_units_skips_degenerate_segments() {
        let segments = vec![
            segment(0, None, "[Page 2]"),
            segment(1, Some("COLLEGE"), "OpenStax provides free, peer-reviewed, openly licensed \
                 textbooks used by students and instructors across many institutions."),
            segment(2, None, ". . . . . 227 8.1 Overview of Photosynthesis . . . . . . . . . ."),
            segment(3, None, "Mitochondria produce ATP through respiration and the citric acid \
                 cycle powers cellular work with the energy stored in its bonds."),
        ];

        let units = build_units(&segments, &ExistingArtifacts::default()).unwrap();
        let used: Vec<&str> = units.iter().map(|unit| unit.segment_id.as_str()).collect();
        let expected = ["seg-1", "seg-3"].repeat(ArtifactType::ALL.len());
        assert_eq!(used.len(), expected.len());

        for artifact_type in ArtifactType::ALL.iter() {
            let contexts: Vec<&str> = units
                .iter()
                .filter(|unit| unit.artifact_type == *artifact_type)
                .map(|unit| unit.context.as_str())
                .collect();
            assert_eq!(contexts.len(), 2);
            assert!(contexts[0].contains("OpenStax provides free"));
            assert!(contexts[1].contains("Mitochondria produce ATP"));
        }
    }

    #[test]
    fn test_build_units_errors_when_everything_degenerate() {
        let segments = vec![
            segment(0, None, "[Page 2]"),
            segment(1, None, "1 2 3 4"),
            segment(2, None, "  "),
        ];
        let result = build_units(&segments, &ExistingArtifacts::default());
        let error = match result {
            Ok(_) => panic!("expected all-degenerate segments to error"),
            Err(error) => error,
        };
        assert!(error.contains("No usable segments"));
    }
}
