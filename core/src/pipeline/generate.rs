//! Cloud-hosted artifact generation: one model turn per unit, no retries;
//! merged types assemble deterministically from per-segment sections.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::{PipelineError, PipelineResult, Stop};

use serde::{Deserialize, Serialize};

use crate::logging::{self, RunLogs};
use crate::provider::{ArtifactBackend, GenerateRequest};
use crate::schema::{ArtifactType, Segment, TypeProgress};

use super::validate::{self, ItemVerdict};

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

struct ArtifactSpec {
    task: &'static str,
    example: Option<&'static str>,
    temperature: Option<f32>,
    max_tokens: Option<i32>,
    items_field: Option<&'static str>,
    schema: fn() -> serde_json::Value,
}

fn spec(artifact_type: &ArtifactType) -> &'static ArtifactSpec {
    static MCQ: ArtifactSpec = ArtifactSpec {
        task: "Write multiple-choice questions testing analysis, application, or evaluation \
               of the material below. Write as many as the material genuinely supports — \
               cover its key ideas, and never pad with shallow or near-identical questions. \
               For every question:\n\
               - Exactly 4 answer options in plain text: no lettering, numbering, or bullet marks.\n\
               - Options must be mutually exclusive, comparable in length, plausible but clearly\n\
               wrong to someone who knows the material.\n\
               - Exactly one correct answer, written verbatim as one of the options.\n\
               - Vary the position of the correct answer across questions.\n\
               - One-sentence explanation citing the supporting fact.\n\
               - Do not repeat near-identical questions.",
        example: Some(MCQ_EXAMPLE),
        temperature: None,
        max_tokens: None,
        items_field: Some("questions"),
        schema: mcq_schema,
    };
    static ESSAY: ArtifactSpec = ArtifactSpec {
        task: "Write essay questions that require students to explain, compare, or evaluate \
               ideas from the material below. Write as many as the material genuinely supports. \
               Each needs clear instructions and a concise suggested answer grounded in the material.",
        example: None,
        temperature: None,
        max_tokens: None,
        items_field: Some("questions"),
        schema: essay_schema,
    };
    static COMPLETION: ArtifactSpec = ArtifactSpec {
        task: "Write fill-in-the-blank statements drawn from the material below. Mark each blank \
               with ____________. Write as many as the material genuinely supports. The expected \
               answer must appear word-for-word in the material. Add a short hint per statement.",
        example: None,
        temperature: None,
        max_tokens: None,
        items_field: Some("items"),
        schema: completion_schema,
    };
    static SUMMARY: ArtifactSpec = ArtifactSpec {
        task: "Summarize this slice of the material: a short title, a focused paragraph capturing \
               its main ideas.",
        example: None,
        temperature: Some(0.3),
        max_tokens: Some(700),
        items_field: None,
        schema: summary_schema,
    };
    static MINDMAP: ArtifactSpec = ArtifactSpec {
        task: "Extract the topic of this slice of the material and its major branches, each with short child concepts drawn from the material.",
        example: None,
        temperature: Some(0.4),
        max_tokens: Some(900),
        items_field: None,
        schema: mindmap_schema,
    };
    match artifact_type {
        ArtifactType::MultipleChoiceQuiz => &MCQ,
        ArtifactType::EssayQuiz => &ESSAY,
        ArtifactType::CompletionQuiz => &COMPLETION,
        ArtifactType::Summary => &SUMMARY,
        ArtifactType::MindMap => &MINDMAP,
    }
}

fn temperature_for(artifact_type: &ArtifactType, params: &GenerationParams) -> f32 {
    spec(artifact_type)
        .temperature
        .unwrap_or(params.temperature)
}

fn max_tokens_for(artifact_type: &ArtifactType, params: &GenerationParams) -> i32 {
    match spec(artifact_type).max_tokens {
        Some(cap) => params.max_tokens.min(cap),
        None => params.max_tokens,
    }
}

/// Beyond this many segments per type, sample evenly instead of fanning out.
const MAX_UNITS_PER_TYPE: usize = 48;

#[derive(Clone, Debug)]
pub struct GenerationTick {
    pub artifact_type: Option<ArtifactType>,
    pub done: usize,
    pub total: usize,
    pub per_type: Vec<TypeProgress>,
    pub types_done: usize,
}

pub type ProgressFn = Arc<dyn Fn(GenerationTick) + Send + Sync>;

/// When `Some`, artifacts route here as each unit completes and nothing is
/// returned; when `None`, all artifacts return as `pending`.
pub type PersistFn = Arc<
    dyn Fn(Vec<PendingArtifact>) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

fn per_type_progress(
    completed_per_type: &[AtomicUsize],
    totals_per_type: &[usize; ArtifactType::ALL.len()],
) -> Vec<TypeProgress> {
    ArtifactType::ALL
        .iter()
        .enumerate()
        .map(|(index, ty)| TypeProgress {
            artifact_type: ty.clone(),
            done: completed_per_type[index].load(Ordering::Relaxed),
            total: totals_per_type[index],
        })
        .collect()
}

pub struct PendingArtifact {
    pub artifact_type: ArtifactType,
    pub source: String,
    pub content: String,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RunTelemetry {
    pub requests: usize,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub backend_errors: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ExistingArtifacts {
    pub done_items: std::collections::HashSet<(String, String)>,
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
    type_index: usize,
    artifact_type: ArtifactType,
    segment_id: String,
    context: String,
    seed_offset: u64,
    skip: bool,
}

fn build_units(segments: &[Segment], existing: &ExistingArtifacts) -> PipelineResult<Vec<Unit>> {
    if segments.is_empty() {
        return Err(PipelineError::NoSegments);
    }

    let usable: Vec<&Segment> = segments
        .iter()
        .filter(|segment| !is_degenerate_segment(segment) && !is_non_teachable_segment(segment))
        .collect();
    if usable.is_empty() {
        return Err(PipelineError::NoUsableSegments);
    }

    let indices = pick_indices(usable.len(), MAX_UNITS_PER_TYPE);
    let contexts: Vec<(String, String, u64)> = indices
        .iter()
        .map(|&segment_index| {
            let segment = usable[segment_index];
            let context = match &segment.heading {
                Some(heading) => format!("[Section: {heading}]\n{}", segment.text),
                None => segment.text.clone(),
            };
            (segment.id.clone(), context, segment.position as u64 + 17)
        })
        .collect();

    let mut units = Vec::new();
    for (type_index, artifact_type) in ArtifactType::ALL.iter().enumerate() {
        for (segment_id, context, seed_offset) in &contexts {
            units.push(Unit {
                type_index,
                artifact_type: artifact_type.clone(),
                segment_id: segment_id.clone(),
                context: context.clone(),
                seed_offset: *seed_offset,
                skip: existing.unit_done(artifact_type, segment_id),
            });
        }
    }
    Ok(units)
}

/// Content-based and format-agnostic: noise leaves share almost no real words.
fn is_degenerate_segment(segment: &Segment) -> bool {
    fn meaningful_words(text: &str) -> usize {
        text.split_whitespace()
            .filter(|word| word.chars().any(char::is_alphabetic))
            .count()
    }

    let text = segment.text.trim();
    text.is_empty() || meaningful_words(text) < MIN_CONTENT_WORDS
}

const MIN_CONTENT_WORDS: usize = 10;

/// Book-furniture headings carry no teachable material. Heading-only match with
/// no publisher names keeps the filter valid across publishers.
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

/// Licensing indicators in a segment's opening text; kept publisher-generic.
const FRONT_MATTER_TEXT_MARKERS: &[&str] = &[
    "copyright",
    "©",
    "creative commons",
    "licensed under",
    "all rights reserved",
    "library of congress",
    "isbn",
];

/// Front matter would make the model reply to nothing or fake an answer, so it
/// stays in the worksheet but never reaches generation.
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
    // Only the opening slice is inspected.
    let lead: String = segment
        .text
        .chars()
        .take(800)
        .collect::<String>()
        .to_lowercase();
    FRONT_MATTER_TEXT_MARKERS
        .iter()
        .any(|marker| lead.contains(marker))
}

fn pick_indices(total: usize, cap: usize) -> Vec<usize> {
    if total <= cap {
        return (0..total).collect();
    }
    let stride = total as f64 / cap as f64;
    (0..cap)
        .map(|index| (index as f64 * stride).floor() as usize)
        .collect()
}

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
    let task = spec(artifact_type);
    let mut prompt = String::from(task.task);
    if let Some(example) = task.example {
        prompt.push('\n');
        prompt.push_str(example);
    }
    format!("{prompt}\n\nSource material:\n\"\"\"\n{context}\n\"\"\"")
}

fn str_field() -> serde_json::Value {
    serde_json::json!({ "type": "string" })
}

fn string_array(min_items: usize, max_items: usize) -> serde_json::Value {
    serde_json::json!({
        "type": "array",
        "items": str_field(),
        "minItems": min_items,
        "maxItems": max_items,
    })
}

fn object_schema(properties: serde_json::Value, required: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn array_schema(items: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "type": "array", "items": items })
}

fn mcq_schema() -> serde_json::Value {
    object_schema(
        serde_json::json!({ "questions": array_schema(mcq_question_schema()) }),
        &["questions"],
    )
}

fn essay_schema() -> serde_json::Value {
    object_schema(
        serde_json::json!({
            "questions": array_schema(object_schema(
                serde_json::json!({
                    "question": str_field(),
                    "instructions": str_field(),
                    "model_answer": str_field(),
                }),
                &["question", "instructions", "model_answer"],
            ))
        }),
        &["questions"],
    )
}

fn completion_schema() -> serde_json::Value {
    object_schema(
        serde_json::json!({
            "items": array_schema(object_schema(
                serde_json::json!({
                    "sentence": str_field(),
                    "answer": str_field(),
                    "hint": str_field(),
                }),
                &["sentence", "answer", "hint"],
            ))
        }),
        &["items"],
    )
}

fn summary_schema() -> serde_json::Value {
    object_schema(
        serde_json::json!({
            "title": str_field(),
            "summary": str_field(),
            "key_points": string_array(3, 5),
        }),
        &["title", "summary", "key_points"],
    )
}

fn mindmap_schema() -> serde_json::Value {
    object_schema(
        serde_json::json!({
            "topic": str_field(),
            "branches": serde_json::json!({
                "type": "array",
                "minItems": 1,
                "maxItems": 6,
                "items": object_schema(
                    serde_json::json!({
                        "label": str_field(),
                        "children": string_array(1, 6),
                    }),
                    &["label", "children"],
                ),
            }),
        }),
        &["topic", "branches"],
    )
}

fn mcq_question_schema() -> serde_json::Value {
    object_schema(
        serde_json::json!({
            "question": str_field(),
            "options": serde_json::json!({
                "type": "array",
                "minItems": 4,
                "maxItems": 4,
                "items": str_field(),
            }),
            "answer": str_field(),
            "explanation": str_field(),
        }),
        &["question", "options", "answer", "explanation"],
    )
}

pub fn schema_for(artifact_type: &ArtifactType) -> serde_json::Value {
    (spec(artifact_type).schema)()
}

fn items_field(artifact_type: &ArtifactType) -> Option<&'static str> {
    spec(artifact_type).items_field
}

/// Tolerates prose around the JSON object: falls back to the first-`{` to
/// last-`}` substring.
fn parse_json_anyhow(raw: &str) -> PipelineResult<serde_json::Value> {
    let trimmed = raw.trim();
    match serde_json::from_str(trimmed) {
        Ok(value) => Ok(value),
        Err(first) => {
            if let (Some(open), Some(close)) = (trimmed.find('{'), trimmed.rfind('}')) {
                if open < close {
                    let slice = &trimmed[open..=close];
                    return serde_json::from_str(slice).map_err(|second| {
                        PipelineError::Failed(format!(
                            "{first}; also failed on the extracted object: {second}"
                        ))
                    });
                }
            }
            Err(PipelineError::Failed(first.to_string()))
        }
    }
}

struct ValidatedOutput {
    items: Vec<String>,
    rejection_reasons: Vec<String>,
}

fn validate_unit_output(
    artifact_type: &ArtifactType,
    raw: &str,
    source_segment: &str,
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

    match items_field(artifact_type) {
        Some(field) => {
            let Some(entries) = value.get(field).and_then(serde_json::Value::as_array) else {
                return rejected(&mut rejection_reasons, format!("missing \"{field}\" array"));
            };

            let mut items = Vec::new();
            for entry in entries {
                let verdict = match artifact_type {
                    ArtifactType::MultipleChoiceQuiz => validate::validate_mcq_item(entry),
                    ArtifactType::EssayQuiz => validate::validate_essay_item(entry),
                    _ => validate::validate_completion_item(entry, source_segment),
                };

                match verdict {
                    ItemVerdict::Accepted(accepted) => items.push(accepted.to_string()),
                    ItemVerdict::Rejected(reason) => rejection_reasons.push(reason),
                }
            }
            ValidatedOutput {
                items,
                rejection_reasons,
            }
        }
        None => {
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

fn outcome_verdict(validated: &ValidatedOutput) -> &'static str {
    if validated.items.is_empty() && validated.rejection_reasons.is_empty() {
        "skipped"
    } else if validated.items.is_empty() {
        "rejected"
    } else {
        "ok"
    }
}

struct UnitOutcome {
    items: Vec<String>,
    requests_made: usize,
    tokens_in: u64,
    tokens_out: u64,
    backend_error: Option<String>,
}

fn log_request(logs: Option<&RunLogs>, unit: &Unit, attempt: usize, request: &GenerateRequest) {
    if let Some(logs) = logs {
        let record = serde_json::json!({
            "timestamp": logging::rfc3339_utc(),
            "unit_type": unit.artifact_type.to_db(),
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
        let label = format!(
            "{}_seed{}_attempt{}",
            logging::sanitize_label(&request.schema_name),
            request.seed,
            attempt
        );
        logs.write_json("generation", &label, &record);
    }
}

fn log_response(
    logs: Option<&RunLogs>,
    unit: &Unit,
    attempt: usize,
    seed: u64,
    record: &serde_json::Value,
) {
    if let Some(logs) = logs {
        let label = format!(
            "{}_seed{}_attempt{}_response",
            logging::sanitize_label(unit.artifact_type.to_db()),
            seed,
            attempt
        );
        logs.write_json("generation", &label, record);
    }
}

async fn run_unit(
    backend: &Arc<dyn ArtifactBackend>,
    unit: &Unit,
    params: &GenerationParams,
    logs: Option<&RunLogs>,
) -> UnitOutcome {
    let mut requests_made = 0usize;
    let mut tokens_in = 0u64;
    let mut tokens_out = 0u64;

    let system = system_prompt().to_string();
    let schema_name = unit.artifact_type.to_db().to_string();
    let seed = params.seed.wrapping_add(unit.seed_offset);
    let request = GenerateRequest {
        system: system.clone(),
        user: user_prompt(&unit.artifact_type, &unit.context),
        schema_name: schema_name.clone(),
        schema: schema_for(&unit.artifact_type),
        temperature: temperature_for(&unit.artifact_type, params),
        max_tokens: max_tokens_for(&unit.artifact_type, params),
        seed,
    };

    log_request(logs, unit, 0, &request);
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
                logs,
                unit,
                0,
                seed,
                &serde_json::json!({
                    "timestamp": logging::rfc3339_utc(),
                    "unit_type": unit.artifact_type.to_db(),
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

    // One turn per unit, no retries; an empty reply is a clean skip, not an error.
    let validated = if reply.text.trim().is_empty() {
        ValidatedOutput {
            items: Vec::new(),
            rejection_reasons: Vec::new(),
        }
    } else {
        validate_unit_output(&unit.artifact_type, &reply.text, &unit.context)
    };
    log_response(
        logs,
        unit,
        0,
        seed,
        &serde_json::json!({
            "timestamp": logging::rfc3339_utc(),
            "unit_type": unit.artifact_type.to_db(),
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
        backend_error: None,
    }
}

struct UnitRecord {
    type_index: usize,
    segment_id: String,
    items: Vec<String>,
}

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
    stop: Stop,
) -> PipelineResult<(Vec<PendingArtifact>, RunTelemetry)> {
    stop.check()?;
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

    if let Some(on_progress) = &on_progress {
        on_progress(GenerationTick {
            artifact_type: None,
            done: 0,
            total: total_units,
            per_type: per_type_progress(&completed_per_type, &totals_per_type),
            types_done: 0,
        });
    }

    let records: Arc<Mutex<Vec<Option<UnitRecord>>>> =
        Arc::new(Mutex::new((0..total_units).map(|_| None).collect()));
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency.clamp(1, 32)));
    let telemetry = Arc::new(Mutex::new(RunTelemetry::default()));
    let first_backend_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let done_counter = Arc::new(AtomicUsize::new(0));

    let mut tasks = tokio::task::JoinSet::new();
    for (unit_index, unit) in active_units.into_iter().enumerate() {
        let semaphore_clone = semaphore.clone();
        let stop_watch = stop.clone();
        let permit = tokio::select! {
            biased;
            _ = stop_watch.stopped() => {
                tasks.abort_all();
                return Err(PipelineError::Cancelled);
            }
            permit = semaphore_clone.acquire_owned() => {
                permit.map_err(|e| PipelineError::Failed(e.to_string()))?
            }
        };
        let backend = backend.clone();
        let params = params.clone();
        let records = records.clone();
        let telemetry = telemetry.clone();
        let first_backend_error = first_backend_error.clone();
        let done_counter = done_counter.clone();
        let completed_per_type = completed_per_type.clone();
        let on_progress = on_progress.clone();
        let on_persist = on_persist.clone();
        let logs = logs.clone();

        tasks.spawn(async move {
            let outcome = run_unit(&backend, &unit, &params, logs.as_deref()).await;

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
                let artifacts = build_item_artifacts(&unit, &outcome.items);
                if let Some(on_persist) = &on_persist {
                    if !artifacts.is_empty() {
                        on_persist(artifacts).await;
                    }
                } else {
                    records.lock().unwrap()[unit_index] = Some(UnitRecord {
                        type_index: unit.type_index,
                        segment_id: unit.segment_id.clone(),
                        items: outcome.items,
                    });
                }
            } else {
                records.lock().unwrap()[unit_index] = Some(UnitRecord {
                    type_index: unit.type_index,
                    segment_id: unit.segment_id.clone(),
                    items: outcome.items,
                });
            }

            let finished = done_counter.fetch_add(1, Ordering::Relaxed) + 1;
            completed_per_type[unit.type_index].fetch_add(1, Ordering::Relaxed);
            let per_type = per_type_progress(&completed_per_type, &totals_per_type);
            let types_done = per_type
                .iter()
                .filter(|progress| progress.total > 0 && progress.done == progress.total)
                .count();

            if let Some(on_progress) = &on_progress {
                on_progress(GenerationTick {
                    artifact_type: Some(unit.artifact_type.clone()),
                    done: finished,
                    total: total_units,
                    per_type,
                    types_done,
                });
            }

            drop(permit);
        });
    }

    // Abort in-flight requests promptly instead of waiting out the timeout.
    loop {
        tokio::select! {
            biased;
            _ = stop.stopped() => {
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
                return Err(PipelineError::Cancelled);
            }
            result = tasks.join_next() => {
                match result {
                    Some(Ok(())) => {}
                    Some(Err(error)) => {
                        tasks.abort_all();
                        while tasks.join_next().await.is_some() {}
                        return Err(if error.is_cancelled() {
                            PipelineError::Cancelled
                        } else {
                            PipelineError::Failed(format!(
                                "Generation task failed: {error}"
                            ))
                        });
                    }
                    None => break,
                }
            }
        }
    }
    stop.check()?;

    let telemetry = *telemetry.lock().unwrap();
    let records: Vec<Option<UnitRecord>> = records.lock().unwrap().drain(..).collect();

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
                on_persist(vec![artifact]).await;
            } else {
                pending.push(artifact);
            }
        }
    }

    // Every unit failing to reach the provider is an outage, not empty success.
    if telemetry.backend_errors > 0 && telemetry.backend_errors == total_units {
        return Err(PipelineError::Failed(String::from(
            "Artifacts generation failed. Check that a provider is configured and reachable.",
        )));
    }

    Ok((pending, telemetry))
}

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

fn first_text(sections: &[(String, serde_json::Value)], field: &str, fallback: &str) -> String {
    sections
        .iter()
        .find_map(|(_, value)| value.get(field).and_then(serde_json::Value::as_str))
        .filter(|text| !text.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn collect_texts(
    sections: &[(String, serde_json::Value)],
    field: &str,
    cap: usize,
    dedupe: bool,
) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (_, value) in sections {
        let texts = match value.get(field) {
            Some(serde_json::Value::String(text)) => vec![text.as_str()],
            Some(serde_json::Value::Array(items)) => {
                items.iter().filter_map(serde_json::Value::as_str).collect()
            }
            _ => Vec::new(),
        };
        for text in texts {
            let trimmed = text.trim();
            if trimmed.is_empty() || (dedupe && !seen.insert(trimmed.to_string())) {
                continue;
            }
            out.push(trimmed.to_string());
            if out.len() >= cap {
                return out;
            }
        }
    }
    out
}

fn merge_sections(
    artifact_type: &ArtifactType,
    sections: &[(String, serde_json::Value)],
) -> Option<String> {
    match artifact_type {
        ArtifactType::Summary => Some(
            serde_json::json!({
                "title": first_text(sections, "title", "Worksheet summary"),
                "summary": collect_texts(sections, "summary", usize::MAX, false).join("\n\n"),
                "key_points": collect_texts(sections, "key_points", 12, true),
            })
            .to_string(),
        ),
        ArtifactType::MindMap => {
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
                    "topic": first_text(sections, "topic", "Overview"),
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
            segment(
                1,
                Some("COLLEGE"),
                "OpenStax provides free, peer-reviewed, openly licensed \
                 textbooks used by students and instructors across many institutions.",
            ),
            segment(
                2,
                None,
                ". . . . . 227 8.1 Overview of Photosynthesis . . . . . . . . . .",
            ),
            segment(
                3,
                None,
                "Mitochondria produce ATP through respiration and the citric acid \
                 cycle powers cellular work with the energy stored in its bonds.",
            ),
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
        assert!(error.to_string().contains("No usable segments"));
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

        assert!(is_non_teachable_segment(&segment(
            1,
            Some("Table of Contents"),
            "Chapter 1 ..."
        )));
        assert!(is_non_teachable_segment(&segment(
            2,
            Some("Preface"),
            "This book introduces..."
        )));
        assert!(is_non_teachable_segment(&segment(
            3,
            None,
            "ISBN 978-0-000-00000-0"
        )));

        let chapter = segment(
            4,
            Some("The Cell"),
            "Cells are the fundamental unit of life. All organisms are composed of cells \
             that carry out the processes of life.",
        );
        assert!(!is_non_teachable_segment(&chapter));

        let long = format!(
            "{} The full body of this chapter goes on at length. {} index",
            "Metabolism converts nutrients into usable energy.".repeat(40),
            "See also".repeat(3)
        );
        assert!(!is_non_teachable_segment(&segment(
            5,
            Some("Metabolism"),
            &long
        )));
    }

    #[test]
    fn test_build_units_skips_front_matter_and_degenerate_segments() {
        let segments = vec![
            segment(0, None, "[Page 2]"),
            segment(
                1,
                Some("COLLEGE"),
                "OpenStax provides free, peer-reviewed, openly licensed \
                 textbooks used by students and instructors across many institutions.",
            ),
            segment(
                2,
                None,
                ". . . . . 227 8.1 Overview of Photosynthesis . . . . . . . . . .",
            ),
            segment(
                3,
                None,
                "Mitochondria produce ATP through respiration and the citric acid \
                 cycle powers cellular work with the energy stored in its bonds.",
            ),
            segment(
                4,
                Some("Colophon"),
                "© Rice University. Licensed under a Creative Commons \
                 Attribution license. Provide attribution on every page when redistributing.",
            ),
        ];

        let units = build_units(&segments, &ExistingArtifacts::default()).unwrap();
        let used: Vec<&str> = units.iter().map(|unit| unit.segment_id.as_str()).collect();
        let expected = ["seg-1", "seg-3"].repeat(ArtifactType::ALL.len());
        assert_eq!(used.len(), expected.len());
        assert!(
            !used.contains(&"seg-4"),
            "front matter must never reach units"
        );
        assert!(!used.contains(&"seg-0") && !used.contains(&"seg-2"));
    }

    #[test]
    fn test_user_prompt_has_no_fixed_count() {
        for artifact_type in ArtifactType::ALL.iter() {
            let prompt = user_prompt(artifact_type, "context");
            let lowered = prompt.to_lowercase();
            assert!(
                ![
                    "exactly 2 questions",
                    "exactly 3 questions",
                    "exactly 4 questions",
                    "exactly 5 questions",
                    "exactly 6 questions"
                ]
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
        let wrapped =
            "Sure — here is the response:\n```json\n{\"questions\":[]}\n```\nHope this helps.";
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
        );
        assert!(validated.items.is_empty());
        assert!(
            validated.rejection_reasons.is_empty(),
            "silent skip, not a rejection"
        );
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

        let outcome = tauri::async_runtime::block_on(run_unit(
            &backend,
            &unit,
            &GenerationParams::default(),
            None,
        ));

        assert!(outcome.items.is_empty());
        assert_eq!(outcome.requests_made, 1, "no retry on a blank reply");
        assert!(
            outcome.backend_error.is_none(),
            "blank reply is not a backend error"
        );
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

        let outcome = tauri::async_runtime::block_on(run_unit(
            &backend,
            &unit,
            &GenerationParams::default(),
            None,
        ));

        assert_eq!(outcome.items.len(), 1);
        assert_eq!(outcome.requests_made, 1);
        assert!(outcome.backend_error.is_none());
    }
}
