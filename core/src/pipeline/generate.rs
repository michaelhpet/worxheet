//! Cloud-hosted artifact generation.
//!
//! Every segment feeds one request per artifact type; requests run under a
//! semaphore so bulk generation stays inside provider rate limits. Each unit
//! gets exactly one model turn — a failed turn (empty reply, malformed or
//! hallucinated output) drops that segment's items rather than retrying.
//! Summary and MindMap are assembled deterministically from per-segment
//! sections, so coverage is exhaustive.

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
        .filter(|segment| !is_degenerate_segment(segment) && !is_non_teachable_segment(segment))
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

/// Generic, publisher-agnostic book-furniture headings: title pages, tables of
/// contents, prefaces, and the like carry no teachable material. Matched on
/// the segment heading only; no publisher names or other variable tokens are
/// encoded so the filter stays valid across publishers and material types.
const FRONT_MATTER_HEADING_MARKERS: &[&str] = &[
    "table of contents",
    "contents",
    "preface",
    "foreword",
    "acknowledg",
    "about this book",
    "about the author",
    "about the textbook",
    "how to use",
    "welcome to",
    "index",
    "colophon",
];

/// Licensing / copyright indicators in the opening text of a segment, used to
/// catch filing pages that carry their own headings (e.g. the publisher's
/// imprint page). Kept generic — no publisher or product names.
const FRONT_MATTER_TEXT_MARKERS: &[&str] = &[
    "copyright",
    "©",
    "creative commons",
    "licensed under",
    "all rights reserved",
    "library of congress",
    "isbn",
];

/// Front matter (title/imprint/TOC/preface pages, licensing boilerplate) holds
/// no educational content and would otherwise make the model reply to nothing
/// or, worse, fake an answer. These segments stay in the worksheet (searchable
/// history) but never reach generation.
fn is_non_teachable_segment(segment: &Segment) -> bool {
    if let Some(heading) = &segment.heading {
        let heading = heading.to_lowercase();
        if FRONT_MATTER_HEADING_MARKERS
            .iter()
            .any(|marker| heading.contains(marker))
        {
            return true;
        }
    }
    // Only the opening slice is inspected so a chapter that mentions "index"
    // or "copyright" deep in its body is not flagged.
    let lead: String = segment.text.chars().take(800).collect::<String>().to_lowercase();
    FRONT_MATTER_TEXT_MARKERS
        .iter()
        .any(|marker| lead.contains(marker))
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
     5. Reply with exactly one JSON object matching the required schema and nothing else.\n\
     6. If the source material is front matter, licensing/copyright text, or other non-core\n\
        content (title pages, tables of contents, prefaces, forewords, colophons), reply with\n\
        an empty collection (e.g. {\"questions\": []}) instead of inventing or padding content."
}

const MCQ_EXAMPLE: &str = r#"Example question object:
{"question":"During aerobic respiration, where does the citric acid cycle occur?",
 "options":["Mitochondrial matrix","Cell nucleus","Ribosome","Golgi apparatus"],
 "answer":"Mitochondrial matrix",
 "explanation":"The cycle runs in the matrix, producing NADH for oxidative phosphorylation."}"#;

fn user_prompt(artifact_type: &ArtifactType, context: &str) -> String {
    let task = match artifact_type {
        ArtifactType::MultipleChoiceQuiz => format!(
            "Write multiple-choice questions testing analysis, application, or evaluation \
             of the material below. Write as many as the material genuinely supports — \
             cover its key ideas, and never pad with shallow or near-identical questions. \
             For every question:\n\
             - Exactly 4 answer options in plain text: no lettering, numbering, or bullet marks.\n\
             - Options must be mutually exclusive, comparable in length, plausible but clearly\n\
             wrong to someone who knows the material. Never offer options like \"all of the above\".\n\
             - Exactly one correct answer, written verbatim as one of the options.\n\
             - Vary the position of the correct answer across questions.\n\
             - One-sentence explanation citing the supporting fact.\n\
             - Do not repeat near-identical questions.\n{MCQ_EXAMPLE}"
        ),
        ArtifactType::EssayQuiz => String::from(
            "Write essay questions that require students to explain, compare, or evaluate \
             ideas from the material below. Write as many as the material genuinely supports. \
             Each needs clear instructions and a concise model answer grounded in the material.",
        ),
        ArtifactType::CompletionQuiz => String::from(
            "Write fill-in-the-blank statements drawn from the material below. Mark each blank \
             with ____________. Write as many as the material genuinely supports. The expected \
             answer must appear word-for-word in the material. Add a short hint per statement.",
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

pub fn schema_for(artifact_type: &ArtifactType) -> serde_json::Value {
    match artifact_type {
        ArtifactType::MultipleChoiceQuiz => serde_json::json!({
            "type": "object",
            "properties": { "questions": mcq_questions_schema() },
            "required": ["questions"],
            "additionalProperties": false,
        }),
        ArtifactType::EssayQuiz => serde_json::json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
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

fn mcq_questions_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "array",
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

/// Parse the model reply, tolerating prose/markdown around the JSON object
/// some servers emit beyond what the client's fence stripping removes: fall
/// back to the substring between the first `{` and the last `}`.
fn parse_json_anyhow(raw: &str) -> Result<serde_json::Value, String> {
    let trimmed = raw.trim();
    match serde_json::from_str(trimmed) {
        Ok(value) => Ok(value),
        Err(first) => {
            if let (Some(open), Some(close)) = (trimmed.find('{'), trimmed.rfind('}')) {
                if open < close {
                    let slice = &trimmed[open..=close];
                    return serde_json::from_str(slice).map_err(|second| {
                        format!("{first}; also failed on the extracted object: {second}")
                    });
                }
            }
            Err(first.to_string())
        }
    }
}

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
    let value: serde_json::Value = match parse_json_anyhow(raw) {
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

/// Human label for a unit's outcome: delivered items, rejected on content, or
/// deliberately skipped (front matter / empty reply).
fn outcome_verdict(validated: &ValidatedOutput) -> &'static str {
    if validated.items.is_empty() && validated.rejection_reasons.is_empty() {
        "skipped"
    } else if validated.items.is_empty() {
        "rejected"
    } else {
        "ok"
    }
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
    let backend_error: Option<String> = None;

    let system = system_prompt().to_string();
    let schema_name = unit.artifact_type.to_db().to_string();
    let schema = schema_for(&unit.artifact_type);
    let base_seed = params.seed.wrapping_add(unit.seed_offset);
    let base_user = user_prompt(&unit.artifact_type, &unit.context);

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

    let request = make_request(base_seed, base_user.clone());
    log_request(0, &request);
    let started = std::time::Instant::now();
    let reply = match backend.generate_json(&request).await {
        Ok(reply) => {
            requests_made += 1;
            tokens_in += approximate_tokens(&system) + approximate_tokens(&request.user);
            tokens_out += approximate_tokens(&reply.text);
            reply
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

    // One turn per unit: no retries. A failed turn (empty reply, invalid JSON,
    // low grounding) simply drops that segment's items.
    let validated = if reply.text.trim().is_empty() {
        // The provider returned nothing: the model declining front matter or an
        // anomalous empty generation. Either way a clean skip — one turn was
        // spent, nothing is fabricated, and finish_reason/refusal below explain
        // the emptiness. Not a backend error and not retried.
        ValidatedOutput {
            items: Vec::new(),
            rejection_reasons: Vec::new(),
        }
    } else {
        validate_unit_output(
            &unit.artifact_type,
            &reply.text,
            &unit.context,
            &seen_questions.lock().unwrap(),
        )
    };
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
            "verdict": outcome_verdict(&validated),
            "field": reply.field_source,
            "finish_reason": reply.finish_reason,
            "refusal": reply.refusal,
            "raw": reply.text,
            "rejection_reasons": validated.rejection_reasons.clone(),
        }),
    );

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

    if validated.items.is_empty() && !validated.rejection_reasons.is_empty() {
        eprintln!(
            "[{}] [pipeline] dropping unit ({schema_name}): {}",
            logging::rfc3339_utc(),
            validated.rejection_reasons[0]
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

    #[test]
    fn test_is_non_teachable_segment_filters_book_furniture() {
        let colophon = segment(
            0,
            Some("OpenStax"),
            "© 2018 Rice University. Textbook content produced by this publisher is \
             licensed under a Creative Commons Attribution license. Download for free at \
             the publisher website and provide attribution on every page.",
        );
        assert!(is_non_teachable_segment(&colophon));

        assert!(is_non_teachable_segment(&segment(1, Some("Table of Contents"), "Chapter 1 ...")));
        assert!(is_non_teachable_segment(&segment(2, Some("Preface"), "This book introduces...")));
        assert!(is_non_teachable_segment(&segment(3, None, "ISBN 978-0-000-00000-0")));

        // Real teaching content must survive, even when a heading looks book-ish.
        let chapter = segment(
            4,
            Some("The Cell"),
            "Cells are the fundamental unit of life. All organisms are composed of cells \
             that carry out the processes of life.",
        );
        assert!(!is_non_teachable_segment(&chapter));
        // A word like "index" deep in a chapter body (beyond the scanned lead)
        // must not trip the filter.
        let long = format!(
            "{} The full body of this chapter goes on at length. {} index",
            "Metabolism converts nutrients into usable energy.".repeat(40),
            "See also".repeat(3)
        );
        assert!(!is_non_teachable_segment(&segment(5, Some("Metabolism"), &long)));
    }

    #[test]
    fn test_build_units_skips_front_matter_and_degenerate_segments() {
        let segments = vec![
            segment(0, None, "[Page 2]"),
            segment(1, Some("COLLEGE"), "OpenStax provides free, peer-reviewed, openly licensed \
                 textbooks used by students and instructors across many institutions."),
            segment(2, None, ". . . . . 227 8.1 Overview of Photosynthesis . . . . . . . . . ."),
            segment(3, None, "Mitochondria produce ATP through respiration and the citric acid \
                 cycle powers cellular work with the energy stored in its bonds."),
            segment(4, Some("Colophon"), "© Rice University. Licensed under a Creative Commons \
                 Attribution license. Provide attribution on every page when redistributing."),
        ];

        let units = build_units(&segments, &ExistingArtifacts::default()).unwrap();
        let used: Vec<&str> = units.iter().map(|unit| unit.segment_id.as_str()).collect();
        let expected = ["seg-1", "seg-3"].repeat(ArtifactType::ALL.len());
        assert_eq!(used.len(), expected.len());
        assert!(!used.contains(&"seg-4"), "front matter must never reach units");
        assert!(!used.contains(&"seg-0") && !used.contains(&"seg-2"));
    }

    #[test]
    fn test_user_prompt_has_no_fixed_count() {
        for artifact_type in ArtifactType::ALL.iter() {
            let prompt = user_prompt(artifact_type, "context");
            let lowered = prompt.to_lowercase();
            assert!(
                !["exactly 2 questions", "exactly 3 questions", "exactly 4 questions",
                  "exactly 5 questions", "exactly 6 questions"]
                    .iter()
                    .any(|phrase| lowered.contains(phrase)),
                "prompt must not fix a question count: {prompt}"
            );
        }
        let mcq = user_prompt(&ArtifactType::MultipleChoiceQuiz, "context");
        assert!(mcq.contains("as many as the material genuinely supports"));
    }

    #[test]
    fn test_question_arrays_are_unbounded() {
        for (artifact_type, array_key) in [
            (ArtifactType::MultipleChoiceQuiz, "questions"),
            (ArtifactType::EssayQuiz, "questions"),
            (ArtifactType::CompletionQuiz, "items"),
        ] {
            let schema = schema_for(&artifact_type);
            let array = &schema["properties"][array_key];
            assert!(
                array.get("minItems").is_none() && array.get("maxItems").is_none(),
                "{artifact_type:?} question array must not bound the count"
            );
        }
    }

    #[test]
    fn test_parse_json_anyhow_recovers_wrapped_json() {
        let wrapped = "Sure — here is the response:\n```json\n{\"questions\":[]}\n```\nHope this helps.";
        assert_eq!(
            parse_json_anyhow(wrapped).unwrap(),
            serde_json::json!({ "questions": [] })
        );
        assert!(parse_json_anyhow("not json at all").is_err());
    }

    #[test]
    fn test_empty_questions_validates_as_clean_skip() {
        let validated = validate_unit_output(
            &ArtifactType::MultipleChoiceQuiz,
            r#"{"questions":[]}"#,
            "source words here",
            &[],
        );
        assert!(validated.items.is_empty());
        assert!(validated.rejection_reasons.is_empty(), "silent skip, not a rejection");
    }

    #[test]
    fn test_blank_response_is_skipped_without_retry() {
        use crate::provider::mock::MockBackend;

        let mock = Arc::new(MockBackend::new(vec![Ok(String::from(""))]));
        let backend: Arc<dyn ArtifactBackend> = mock.clone();
        let unit = Unit {
            type_index: 0,
            artifact_type: ArtifactType::MultipleChoiceQuiz,
            segment_id: String::from("seg-1"),
            context: String::from(
                "The mitochondrion produces ATP through respiration and the citric acid cycle.",
            ),
            seed_offset: 0,
            skip: false,
        };
        let seen_questions = Mutex::new(Vec::<String>::new());

        let outcome = tauri::async_runtime::block_on(run_unit(
            &backend,
            &unit,
            &GenerationParams::default(),
            &seen_questions,
            None,
        ));

        assert!(outcome.items.is_empty());
        assert_eq!(outcome.requests_made, 1, "no retry on a blank reply");
        assert!(outcome.backend_error.is_none(), "blank reply is not a backend error");
        assert_eq!(mock.request_count(), 1);
    }

    #[test]
    fn test_valid_questions_flow_through() {
        use crate::provider::mock::MockBackend;

        let mock = Arc::new(MockBackend::new(vec![Ok(String::from(
            r#"{"questions":[{"question":"Where does the citric acid cycle run?",
                                 "options":["Mitochondrial matrix","Cell nucleus","Ribosome","Golgi apparatus"],
                                 "answer":"Mitochondrial matrix",
                                 "explanation":"The cycle runs in the matrix producing NADH for the electron transport chain."}]}"#,
        ))]));
        let backend: Arc<dyn ArtifactBackend> = mock.clone();
        let unit = Unit {
            type_index: 0,
            artifact_type: ArtifactType::MultipleChoiceQuiz,
            segment_id: String::from("seg-1"),
            context: String::from(
                "The mitochondrion produces ATP through respiration and the citric acid cycle \
                 runs in the mitochondrial matrix producing NADH.",
            ),
            seed_offset: 0,
            skip: false,
        };
        let seen_questions = Mutex::new(Vec::<String>::new());

        let outcome = tauri::async_runtime::block_on(run_unit(
            &backend,
            &unit,
            &GenerationParams::default(),
            &seen_questions,
            None,
        ));

        assert_eq!(outcome.items.len(), 1);
        assert_eq!(outcome.requests_made, 1);
        assert!(outcome.backend_error.is_none());
    }
}
