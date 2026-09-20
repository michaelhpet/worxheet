//! Cloud-hosted artifact generation: one model turn per unit with bounded
//! retries on transport errors. Quiz types fan out per segment; Summary and
//! MindMap cover the whole worksheet in one request each, guided by a
//! deterministic topic skeleton.

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
        task: "Write an article-length summary of the material below. Reply as exactly \
               one JSON object with three keys: \"title\" (a short title string), \
               \"summary\" (the article string), and \"key_points\" (an array of 3 to 5 \
               short takeaway strings). Cover each major topic in document order. \
               Format the `summary` field as Markdown: use `##` headers \
               per major topic, paragraphs for exposition, and ordered/unordered lists \
               or `**bold**`/`*italic*`/inline `code` where they aid readability. \
               Keep Markdown inside the JSON string value only.",
        example: Some(SUMMARY_EXAMPLE),
        temperature: Some(0.3),
        max_tokens: Some(4000),
        items_field: None,
        schema: summary_schema,
    };
    static MINDMAP: ArtifactSpec = ArtifactSpec {
        task: "Extract the overarching topic of the material below and its major branches. \
               Each branch carries a short label and children which are themselves branches. \
               Keep the map tight enough to fit: at most 12 top-level branches, at most 3 \
               levels of nesting, one short label per node. Every branch object must have \
               both \"label\" and \"children\" keys — leaves use \"children\": []. \
               Prefer major topics over exhaustive leaf concepts; consolidate details \
               into their parent label.",
        example: None,
        temperature: Some(0.4),
        max_tokens: Some(8000),
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
        Some(floor) => params.max_tokens.max(floor),
        None => params.max_tokens,
    }
}

/// Beyond this many segments, quiz types sample evenly instead of fanning out.
/// Merged types (Summary/MindMap) cover the whole worksheet in one request each.
const MAX_QUIZ_SEGMENTS: usize = 48;

/// Source-material budget, in approximate tokens, for one worksheet-wide
/// Summary or MindMap request.
const WHOLE_MATERIAL_BUDGET_TOKENS: usize = 8_000;

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
/// returned; when `None`, all artifacts return as `pending`. Persist failures
/// are returned so the caller can log and fall back instead of dropping data.
pub type PersistFn = Arc<
    dyn Fn(
            Vec<PendingArtifact>,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = super::PipelineResult<()>> + Send>>
        + Send
        + Sync,
>;

/// Transport retries per unit (initial attempt + 2 retries). Validation
/// rejects are not retried.
const MAX_GENERATION_ATTEMPTS: usize = 3;

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

fn usable_segments(segments: &[Segment]) -> PipelineResult<Vec<&Segment>> {
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
    Ok(usable)
}

fn build_units(
    segments: &[Segment],
    existing: &ExistingArtifacts,
    only_types: Option<&[ArtifactType]>,
) -> PipelineResult<Vec<Unit>> {
    let usable = usable_segments(segments)?;

    let indices = pick_indices(usable.len(), MAX_QUIZ_SEGMENTS);

    let mut units = Vec::new();
    for &segment_index in &indices {
        let segment = usable[segment_index];
        let context = section_context(segment);
        for artifact_type in ArtifactType::QUIZ.iter() {
            if only_types.is_some_and(|only| !only.contains(artifact_type)) {
                continue;
            }
            units.push(Unit {
                type_index: type_index(artifact_type),
                artifact_type: artifact_type.clone(),
                segment_id: segment.id.clone(),
                context: context.clone(),
                seed_offset: segment.position as u64 + 17,
                skip: existing.unit_done(artifact_type, &segment.id),
            });
        }
    }
    Ok(units)
}

fn section_context(segment: &Segment) -> String {
    match &segment.heading {
        Some(heading) => format!("[Section: {heading}]\n{}", segment.text),
        None => segment.text.clone(),
    }
}

fn type_index(artifact_type: &ArtifactType) -> usize {
    ArtifactType::ALL
        .iter()
        .position(|ty| ty == artifact_type)
        .expect("every artifact type is a member of ALL")
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

struct Topic {
    label: String,
    members: Vec<usize>,
}

fn word_terms(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.len() >= 3)
        .map(|word| word.to_lowercase())
}

fn term_scores(usable: &[&Segment]) -> std::collections::HashMap<String, f32> {
    let mut frequency: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut documents: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for segment in usable {
        let mut seen = std::collections::HashSet::new();
        for term in word_terms(&segment.text)
            .chain(word_terms(segment.heading.as_deref().unwrap_or_default()))
        {
            *frequency.entry(term.clone()).or_default() += 1;
            if seen.insert(term.clone()) {
                *documents.entry(term).or_default() += 1;
            }
        }
    }
    let total = usable.len() as f32;
    frequency
        .iter()
        .map(|(term, count)| {
            let inverse = (total / documents[term] as f32).ln().max(0.0);
            (term.clone(), *count as f32 * inverse)
        })
        .collect()
}

fn extract_topics(
    usable: &[&Segment],
    scores: &std::collections::HashMap<String, f32>,
) -> Vec<Topic> {
    let mut topics: Vec<Topic> = Vec::new();
    for (index, segment) in usable.iter().enumerate() {
        match segment
            .heading
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty())
        {
            Some(heading) => topics.push(Topic {
                label: heading.to_string(),
                members: vec![index],
            }),
            None => match topics.last_mut() {
                Some(topic) => topic.members.push(index),
                None => topics.push(Topic {
                    label: top_term_label(scores),
                    members: vec![index],
                }),
            },
        }
    }
    topics
}

fn top_term_label(scores: &std::collections::HashMap<String, f32>) -> String {
    scores
        .iter()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(term, _)| term.clone())
        .unwrap_or_else(|| String::from("General"))
}

fn select_covering_segments<'a>(
    topics: &[Topic],
    usable: &[&'a Segment],
    scores: &std::collections::HashMap<String, f32>,
    budget_tokens: usize,
) -> Vec<&'a Segment> {
    let member_scores: Vec<f32> = usable
        .iter()
        .map(|segment| {
            word_terms(&segment.text)
                .collect::<std::collections::HashSet<_>>()
                .iter()
                .map(|term| scores.get(term).copied().unwrap_or(0.0))
                .sum()
        })
        .collect();
    let mut picked: Vec<usize> = Vec::new();
    let mut spent = 0usize;
    let mut remaining: Vec<Vec<usize>> = topics
        .iter()
        .map(|topic| {
            let mut members = topic.members.clone();
            members.sort_by(|&a, &b| {
                member_scores[b]
                    .partial_cmp(&member_scores[a])
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            members
        })
        .collect();
    loop {
        let mut progressed = false;
        for members in remaining.iter_mut() {
            while let Some(&index) = members.first() {
                if picked.contains(&index) {
                    members.remove(0);
                    continue;
                }
                let cost = approximate_tokens(&usable[index].text) as usize;
                if spent + cost > budget_tokens && !picked.is_empty() {
                    break;
                }
                spent += cost;
                picked.push(index);
                members.remove(0);
                progressed = true;
                break;
            }
        }
        if !progressed {
            break;
        }
    }
    if picked.is_empty() {
        if let Some(best) = (0..usable.len()).max_by(|&a, &b| {
            member_scores[a]
                .partial_cmp(&member_scores[b])
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            picked.push(best);
        }
    }
    picked.sort_unstable();
    picked.iter().map(|&index| usable[index]).collect()
}

fn empty_collection_example(artifact_type: &ArtifactType) -> &'static str {
    match items_field(artifact_type) {
        Some("items") => "{\"items\": []}",
        _ => "{\"questions\": []}",
    }
}

fn system_prompt_for(artifact_type: &ArtifactType) -> String {
    format!(
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
            an empty collection (e.g. {}) instead of inventing or padding content.",
        empty_collection_example(artifact_type)
    )
}

const MCQ_EXAMPLE: &str = r#"Example question object:
{"question":"During aerobic respiration, where does the citric acid cycle occur?",
 "options":["Mitochondrial matrix","Cell nucleus","Ribosome","Golgi apparatus"],
 "answer":"Mitochondrial matrix",
 "explanation":"The cycle runs in the matrix, producing NADH for oxidative phosphorylation."}"#;

const SUMMARY_EXAMPLE: &str = r###"Example reply shape (all three keys required):
{"title":"Cellular Respiration",
 "summary":"## Overview\nRespiration releases energy from glucose in three stages…",
 "key_points":["Glycolysis splits glucose in the cytosol","The citric acid cycle runs in the mitochondrial matrix"]}"###;

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
    // Recursive nodes via $defs: every branch carries a label and children
    // that are themselves branches, to whatever depth the material warrants.
    // Servers that reject `response_format` fall back to prompt-only JSON.
    serde_json::json!({
        "type": "object",
        "properties": {
            "topic": str_field(),
            "branches": {
                "type": "array",
                "items": { "$ref": "#/$defs/node" },
            },
        },
        "required": ["topic", "branches"],
        "additionalProperties": false,
        "$defs": {
            "node": {
                "type": "object",
                "properties": {
                    "label": str_field(),
                    "children": {
                        "type": "array",
                        "items": { "$ref": "#/$defs/node" },
                    },
                },
                "required": ["label", "children"],
                "additionalProperties": false,
            },
        },
    })
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

/// Providers routinely omit `"children": []` on leaf nodes even when the
/// schema requires it (observed: 76 labels with 32 `children` keys).
/// Normalize absent children to `[]` so a structurally sound map is stored
/// canonically; reject only empty/missing labels or non-array children.
fn normalize_mindmap_node(node: &serde_json::Value) -> Option<serde_json::Value> {
    let label = node.get("label")?.as_str()?;
    if label.trim().is_empty() {
        return None;
    }
    let children = match node.get("children") {
        None => Vec::new(),
        Some(value) => value
            .as_array()?
            .iter()
            .map(normalize_mindmap_node)
            .collect::<Option<Vec<_>>>()?,
    };
    Some(serde_json::json!({ "label": label, "children": children }))
}

/// Validated merged-type output, normalized for storage. `None` means the
/// shape is unusable.
fn normalized_merged_output(
    artifact_type: &ArtifactType,
    value: &serde_json::Value,
) -> Option<serde_json::Value> {
    match artifact_type {
        ArtifactType::Summary => {
            let shape_ok = value
                .get("summary")
                .is_some_and(serde_json::Value::is_string)
                && value
                    .get("key_points")
                    .is_some_and(serde_json::Value::is_array);
            shape_ok.then(|| value.clone())
        }
        ArtifactType::MindMap => {
            if !value
                .get("topic")
                .is_some_and(serde_json::Value::is_string)
            {
                return None;
            }
            let branches = value.get("branches")?.as_array()?;
            let normalized = branches
                .iter()
                .map(normalize_mindmap_node)
                .collect::<Option<Vec<_>>>()?;
            Some(serde_json::json!({
                "topic": value.get("topic"),
                "branches": normalized,
            }))
        }
        _ => None,
    }
}

struct ValidatedOutput {
    items: Vec<String>,
    rejection_reasons: Vec<String>,
}

fn is_truncated(finish_reason: &str) -> bool {
    finish_reason.eq_ignore_ascii_case("length")
}

fn validate_unit_output(
    artifact_type: &ArtifactType,
    raw: &str,
    source_segment: &str,
    finish_reason: &str,
) -> ValidatedOutput {
    fn rejected(reasons: &mut Vec<String>, message: String) -> ValidatedOutput {
        reasons.push(message);
        ValidatedOutput {
            items: Vec::new(),
            rejection_reasons: reasons.clone(),
        }
    }

    let mut rejection_reasons = Vec::new();
    if is_truncated(finish_reason) {
        return rejected(
            &mut rejection_reasons,
            format!(
                "truncated: finish_reason=length; increase max_tokens or reduce input budget ({} input budget)",
                WHOLE_MATERIAL_BUDGET_TOKENS
            ),
        );
    }
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
        None => match normalized_merged_output(artifact_type, &value) {
            Some(normalized) => ValidatedOutput {
                items: vec![normalized.to_string()],
                rejection_reasons,
            },
            None => rejected(
                &mut rejection_reasons,
                format!(
                    "expected a {} object with the documented fields",
                    artifact_type.to_db()
                ),
            ),
        },
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

    let system = system_prompt_for(&unit.artifact_type);
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
    let mut last_error: Option<String> = None;
    let mut reply = None;
    let mut attempts_made = 0usize;
    for attempt in 0..MAX_GENERATION_ATTEMPTS {
        attempts_made += 1;
        match backend.generate_json(&request).await {
            Ok(ok) => {
                reply = Some(ok);
                last_error = None;
                break;
            }
            Err(error) => {
                let retryable = error.is_retryable();
                last_error = Some(format!("{error}"));
                log_response(
                    logs,
                    unit,
                    attempt,
                    seed,
                    &serde_json::json!({
                        "timestamp": logging::rfc3339_utc(),
                        "unit_type": unit.artifact_type.to_db(),
                        "segment_id": unit.segment_id,
                        "attempt": attempt,
                        "elapsed_ms": started.elapsed().as_millis(),
                        "status": "error",
                        "retryable": retryable,
                        "error": format!("{error}"),
                    }),
                );
                eprintln!(
                    "[{}] [pipeline] unit failed ({schema_name}) attempt {attempt}: {error}",
                    logging::rfc3339_utc()
                );
                if !retryable {
                    break;
                }
                if attempt + 1 < MAX_GENERATION_ATTEMPTS {
                    tokio::time::sleep(std::time::Duration::from_millis(
                        200 * (attempt as u64 + 1),
                    ))
                    .await;
                }
            }
        }
    }
    let Some(reply) = reply else {
        requests_made += attempts_made;
        return UnitOutcome {
            items: Vec::new(),
            requests_made,
            tokens_in,
            tokens_out,
            backend_error: last_error,
        };
    };
    requests_made += attempts_made;
    tokens_in += approximate_tokens(&system) + approximate_tokens(&request.user);
    tokens_out += approximate_tokens(&reply.text);
    let validated = if reply.text.trim().is_empty() {
        ValidatedOutput {
            items: Vec::new(),
            rejection_reasons: Vec::new(),
        }
    } else {
        validate_unit_output(
            &unit.artifact_type,
            &reply.text,
            &unit.context,
            &reply.finish_reason,
        )
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

struct UnitDriver {
    backend: Arc<dyn ArtifactBackend>,
    params: GenerationParams,
    concurrency: usize,
    on_persist: Option<PersistFn>,
    on_progress: Option<ProgressFn>,
    logs: Option<Arc<RunLogs>>,
    stop: Stop,
}

impl UnitDriver {
    async fn run(
        &self,
        units: Vec<Unit>,
    ) -> PipelineResult<(Vec<Option<UnitRecord>>, RunTelemetry)> {
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

        if let Some(on_progress) = &self.on_progress {
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
        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.concurrency.clamp(1, 32)));
        let telemetry = Arc::new(Mutex::new(RunTelemetry::default()));
        let first_backend_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let done_counter = Arc::new(AtomicUsize::new(0));

        let mut tasks = tokio::task::JoinSet::new();
        for (unit_index, unit) in active_units.into_iter().enumerate() {
            let semaphore_clone = semaphore.clone();
            let stop_watch = self.stop.clone();
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
            let backend = self.backend.clone();
            let params = self.params.clone();
            let records = records.clone();
            let telemetry = telemetry.clone();
            let first_backend_error = first_backend_error.clone();
            let done_counter = done_counter.clone();
            let completed_per_type = completed_per_type.clone();
            let on_progress = self.on_progress.clone();
            let on_persist = self.on_persist.clone();
            let logs = self.logs.clone();

            tasks.spawn(async move {
                let outcome = run_unit(&backend, &unit, &params, logs.as_deref()).await;
                drop(permit);

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
                            if let Err(error) = on_persist(artifacts).await {
                                eprintln!(
                                    "[{}] [pipeline] incremental persist failed, buffering for final persist: {error}",
                                    logging::rfc3339_utc()
                                );
                                records.lock().unwrap()[unit_index] = Some(UnitRecord {
                                    type_index: unit.type_index,
                                    segment_id: unit.segment_id.clone(),
                                    items: outcome.items,
                                });
                            }
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
            });
        }

        // Abort in-flight requests promptly instead of waiting out the timeout.
        loop {
            tokio::select! {
                biased;
                _ = self.stop.stopped() => {
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
        self.stop.check()?;

        let telemetry = *telemetry.lock().unwrap();
        let records: Vec<Option<UnitRecord>> = records.lock().unwrap().drain(..).collect();
        Ok((records, telemetry))
    }
}

fn build_merged_units(
    segments: &[Segment],
    existing: &ExistingArtifacts,
    only_types: Option<&[ArtifactType]>,
) -> PipelineResult<Vec<Unit>> {
    let usable = usable_segments(segments)?;
    let scores = term_scores(&usable);
    let topics = extract_topics(&usable, &scores);
    let selected =
        select_covering_segments(&topics, &usable, &scores, WHOLE_MATERIAL_BUDGET_TOKENS);
    let source = selected
        .iter()
        .map(|segment| segment.id.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let body = selected
        .iter()
        .map(|segment| section_context(segment))
        .collect::<Vec<_>>()
        .join("\n\n");
    let mindmap_body = format!(
        "Major topics: {}\n\n{body}",
        topics
            .iter()
            .map(|topic| topic.label.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    );

    let mut units = Vec::new();
    for (artifact_type, context, seed_offset) in [
        (ArtifactType::Summary, body.clone(), 0u64),
        (ArtifactType::MindMap, mindmap_body, 1u64),
    ] {
        if only_types.is_some_and(|only| !only.contains(&artifact_type)) {
            continue;
        }
        units.push(Unit {
            type_index: type_index(&artifact_type),
            artifact_type: artifact_type.clone(),
            segment_id: source.clone(),
            context,
            seed_offset,
            skip: existing.unit_done(&artifact_type, &source),
        });
    }
    Ok(units)
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
    only_types: Option<&[ArtifactType]>,
) -> PipelineResult<(Vec<PendingArtifact>, RunTelemetry)> {
    stop.check()?;
    let params = params.unwrap_or_default();
    let driver = UnitDriver {
        backend,
        params,
        concurrency,
        on_persist,
        on_progress,
        logs,
        stop: stop.clone(),
    };

    let units = build_units(segments, &existing, only_types)?;
    let quiz_total = units.iter().filter(|unit| !unit.skip).count();
    let (records, mut telemetry) = driver.run(units).await?;

    let merged_units = build_merged_units(segments, &existing, only_types)?;
    let merged_total = merged_units.iter().filter(|unit| !unit.skip).count();
    let (merged_records, merged_telemetry) = driver.run(merged_units).await?;
    telemetry.requests += merged_telemetry.requests;
    telemetry.tokens_in += merged_telemetry.tokens_in;
    telemetry.tokens_out += merged_telemetry.tokens_out;
    telemetry.backend_errors += merged_telemetry.backend_errors;

    let mut pending = Vec::new();
    if driver.on_persist.is_none() {
        for (type_index, artifact_type) in ArtifactType::ALL.iter().enumerate() {
            for record in records
                .iter()
                .flatten()
                .filter(|record| record.type_index == type_index && !record.items.is_empty())
            {
                for item in &record.items {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(item) {
                        pending.push(PendingArtifact {
                            artifact_type: artifact_type.clone(),
                            source: record.segment_id.clone(),
                            content: value.to_string(),
                        });
                    }
                }
            }
        }
    }
    for record in merged_records.iter().flatten() {
        if let Some(content) = record.items.first() {
            let artifact_type = ArtifactType::ALL[record.type_index].clone();
            let source = record.segment_id.clone();
            let content = content.clone();
            if let Some(on_persist) = &driver.on_persist {
                let artifact = PendingArtifact {
                    artifact_type: artifact_type.clone(),
                    source: source.clone(),
                    content: content.clone(),
                };
                if let Err(error) = on_persist(vec![artifact]).await {
                    eprintln!(
                        "[{}] [pipeline] merged persist failed, buffering for final persist: {error}",
                        logging::rfc3339_utc()
                    );
                    pending.push(PendingArtifact {
                        artifact_type,
                        source,
                        content,
                    });
                }
            } else {
                pending.push(PendingArtifact {
                    artifact_type,
                    source,
                    content,
                });
            }
        }
    }

    // Every unit failing to reach the provider is an outage, not empty success.
    let total_units = quiz_total + merged_total;
    if telemetry.backend_errors > 0 && telemetry.backend_errors == total_units {
        return Err(PipelineError::Failed(String::from(
            "Artifacts generation failed. Check that a provider is configured and reachable.",
        )));
    }
    if telemetry.backend_errors > 0 {
        eprintln!(
            "[{}] [pipeline] partial generation: {}/{} units failed to reach the provider",
            logging::rfc3339_utc(),
            telemetry.backend_errors,
            total_units
        );
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

        let units = build_units(&segments, &ExistingArtifacts::default(), None).unwrap();
        let used: Vec<&str> = units.iter().map(|unit| unit.segment_id.as_str()).collect();
        let expected = ["seg-1", "seg-3"].repeat(ArtifactType::QUIZ.len());
        assert_eq!(used.len(), expected.len());

        for artifact_type in ArtifactType::QUIZ.iter() {
            let contexts: Vec<&str> = units
                .iter()
                .filter(|unit| unit.artifact_type == *artifact_type)
                .map(|unit| unit.context.as_str())
                .collect();
            assert_eq!(contexts.len(), 2);
            assert!(contexts[0].contains("OpenStax provides free"));
            assert!(contexts[1].contains("Mitochondria produce ATP"));
        }
        assert!(
            units
                .iter()
                .all(|unit| ArtifactType::QUIZ.contains(&unit.artifact_type)),
            "merged types never fan out per segment"
        );
    }

    #[test]
    fn test_build_units_errors_when_everything_degenerate() {
        let segments = vec![
            segment(0, None, "[Page 2]"),
            segment(1, None, "1 2 3 4"),
            segment(2, None, "  "),
        ];
        let result = build_units(&segments, &ExistingArtifacts::default(), None);
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

        let units = build_units(&segments, &ExistingArtifacts::default(), None).unwrap();
        let used: Vec<&str> = units.iter().map(|unit| unit.segment_id.as_str()).collect();
        let expected = ["seg-1", "seg-3"].repeat(ArtifactType::QUIZ.len());
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
    fn test_term_scores_rank_distinctive_terms_without_stopwords() {
        let bodies = [
            "Mitochondria produce ATP through respiration and the mitochondria power the cell.",
            "Mitochondria carry their own DNA alongside the energy pathways of mitochondria.",
            "The students read the textbook chapter and the lesson covers the material.",
            "The class reviewed the lesson and the students discussed the chapter.",
            "The book presents the material while the lesson introduces the topic.",
            "The students studied the chapter before the class reviewed the lesson.",
            "The textbook lesson connects the chapter material to the class discussion.",
            "The class examined how the lesson frames the chapter for the students.",
            "The students summarized the material after the lesson ended.",
            "The chapter review helped the class prepare before the lesson.",
        ];
        let segments: Vec<Segment> = bodies
            .iter()
            .enumerate()
            .map(|(i, text)| segment(i as i32, None, text))
            .collect();
        let refs: Vec<&Segment> = segments.iter().collect();
        let scores = term_scores(&refs);
        assert!(
            scores["mitochondria"] > scores["the"],
            "a repeated distinctive term must outrank ubiquitous words"
        );
    }

    #[test]
    fn test_extract_topics_groups_by_heading_in_order() {
        let segments = [
            segment(
                0,
                Some("Respiration"),
                "Mitochondria produce ATP through respiration.",
            ),
            segment(1, None, "The citric acid cycle runs in the matrix."),
            segment(
                2,
                Some("Volcanoes"),
                "Magma pressure builds beneath the crust.",
            ),
        ];
        let refs: Vec<&Segment> = segments.iter().collect();
        let scores = term_scores(&refs);
        let topics = extract_topics(&refs, &scores);
        assert_eq!(topics.len(), 2);
        assert_eq!(topics[0].label, "Respiration");
        assert_eq!(topics[0].members, vec![0, 1]);
        assert_eq!(topics[1].label, "Volcanoes");
        assert_eq!(topics[1].members, vec![2]);
    }

    #[test]
    fn test_select_covering_segments_spreads_budget_in_document_order() {
        let filler =
            "Mitochondria produce ATP through cellular respiration and glycolysis pathways.";
        let segments = [
            segment(0, Some("One"), filler),
            segment(1, Some("Two"), filler),
            segment(2, Some("Three"), filler),
        ];
        let refs: Vec<&Segment> = segments.iter().collect();
        let scores = term_scores(&refs);
        let topics = extract_topics(&refs, &scores);
        let selected = select_covering_segments(&topics, &refs, &scores, 50);
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].id, "seg-0");
        assert_eq!(selected[1].id, "seg-1");
    }

    #[test]
    fn test_build_merged_units_cover_whole_material_in_two_requests() {
        let segments = [
            segment(
                0,
                Some("Respiration"),
                "Mitochondria produce ATP through respiration and the citric acid cycle.",
            ),
            segment(
                1,
                Some("Volcanoes"),
                "Magma pressure builds beneath the crust before volcanic eruptions release ash and lava across the landscape.",
            ),
        ];
        let units = build_merged_units(&segments, &ExistingArtifacts::default(), None).unwrap();
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].artifact_type, ArtifactType::Summary);
        assert_eq!(units[1].artifact_type, ArtifactType::MindMap);
        assert_eq!(units[0].segment_id, units[1].segment_id);
        assert!(units[0].context.contains("Mitochondria"));
        assert!(units[0].context.contains("Magma"));
        assert!(units[1].context.contains("Respiration"));
    }

    #[test]
    fn test_build_merged_units_skip_finished_types() {
        let segments = vec![segment(
            0,
            None,
            "Mitochondria produce ATP through respiration and the citric acid cycle.",
        )];
        let mut existing = ExistingArtifacts::default();
        existing.done_merged.insert(String::from("Summary"));
        let units = build_merged_units(&segments, &existing, None).unwrap();
        let active: Vec<_> = units.iter().filter(|unit| !unit.skip).collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].artifact_type, ArtifactType::MindMap);
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
            "stop",
        );
        assert!(validated.items.is_empty());
        assert!(
            validated.rejection_reasons.is_empty(),
            "silent skip, not a rejection"
        );
    }

    #[test]
    fn test_system_prompt_empty_shape_matches_validator_contract() {
        for (artifact_type, expected_field) in [
            (ArtifactType::MultipleChoiceQuiz, "questions"),
            (ArtifactType::EssayQuiz, "questions"),
            (ArtifactType::CompletionQuiz, "items"),
        ] {
            let prompt = system_prompt_for(&artifact_type);
            assert!(
                prompt.contains(empty_collection_example(&artifact_type)),
                "{artifact_type:?} prompt must show its own empty shape"
            );
            let empty = if expected_field == "items" {
                r#"{"items":[]}"#.to_string()
            } else {
                r#"{"questions":[]}"#.to_string()
            };
            let validated =
                validate_unit_output(&artifact_type, &empty, "source words here", "stop");
            assert!(validated.items.is_empty());
            assert!(
                validated.rejection_reasons.is_empty(),
                "{artifact_type:?} empty reply must skip, not reject"
            );
            let schema = schema_for(&artifact_type);
            assert!(
                schema["properties"][expected_field].is_object(),
                "{artifact_type:?} schema must require \"{expected_field}\""
            );
        }
        assert_eq!(
            empty_collection_example(&ArtifactType::CompletionQuiz),
            "{\"items\": []}"
        );
    }

    #[test]
    fn test_merged_prompts_state_shape_and_size_bounds() {
        let summary = user_prompt(&ArtifactType::Summary, "context");
        for key in ["\"title\"", "\"summary\"", "\"key_points\""] {
            assert!(
                summary.contains(key),
                "summary prompt must name its JSON keys, missing {key}"
            );
        }
        let mindmap = user_prompt(&ArtifactType::MindMap, "context");
        assert!(
            mindmap.contains("12 top-level branches") && mindmap.contains("3"),
            "mindmap prompt must bound breadth and depth"
        );
    }

    #[test]
    fn test_mindmap_leaf_without_children_is_normalized() {
        let validated = validate_unit_output(
            &ArtifactType::MindMap,
            r#"{"topic":"Biology","branches":[{"label":"Cells"},{"label":"Genetics","children":[{"label":"DNA"}]}]}"#,
            "source words here",
            "stop",
        );
        assert!(
            validated.rejection_reasons.is_empty(),
            "leaves without children must not reject: {:?}",
            validated.rejection_reasons
        );
        assert_eq!(validated.items.len(), 1);
        let stored: serde_json::Value = serde_json::from_str(&validated.items[0]).unwrap();
        assert_eq!(stored["branches"][0]["children"], serde_json::json!([]));
        assert_eq!(
            stored["branches"][1]["children"][0]["children"],
            serde_json::json!([])
        );
        assert_eq!(stored["topic"], serde_json::json!("Biology"));
    }

    #[test]
    fn test_mindmap_bad_nodes_still_rejected() {
        for raw in [
            r#"{"topic":"T","branches":[{"label":"  "}]}"#,
            r#"{"topic":"T","branches":[{"label":"A","children":{}}]}"#,
            r#"{"branches":[{"label":"A","children":[]}]}"#,
        ] {
            let validated =
                validate_unit_output(&ArtifactType::MindMap, raw, "source", "stop");
            assert!(
                !validated.rejection_reasons.is_empty(),
                "must reject {raw}"
            );
            assert!(validated.items.is_empty());
        }
    }

    #[test]
    fn test_truncated_reply_rejected_with_length_reason() {
        let validated = validate_unit_output(
            &ArtifactType::Summary,
            r#"{"title":"T","summary":"partial"#,
            "source words here",
            "length",
        );
        assert!(validated.items.is_empty());
        assert!(
            validated
                .rejection_reasons
                .iter()
                .any(|reason| reason.contains("finish_reason=length")),
            "truncation must be explicit, got {:?}",
            validated.rejection_reasons
        );
        assert!(is_truncated("length"));
        assert!(is_truncated("LENGTH"));
        assert!(!is_truncated("stop"));
    }

    #[test]
    fn test_merged_types_allow_larger_output_budget() {
        let params = GenerationParams::default();
        assert_eq!(max_tokens_for(&ArtifactType::Summary, &params), 4000);
        assert_eq!(max_tokens_for(&ArtifactType::MindMap, &params), 8000);
        assert_eq!(
            max_tokens_for(&ArtifactType::MultipleChoiceQuiz, &params),
            params.max_tokens
        );
        let raised = GenerationParams {
            max_tokens: 8000,
            ..GenerationParams::default()
        };
        assert_eq!(max_tokens_for(&ArtifactType::Summary, &raised), 8000);
    }

    #[test]
    fn test_rejected_errors_fail_fast_without_retry() {
        use crate::provider::mock::MockBackend;
        use crate::provider::ProviderError;

        let mock = Arc::new(MockBackend::new(vec![Err(ProviderError::Rejected(
            String::from("401: missing key"),
        ))]));
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
        assert!(outcome.backend_error.is_some());
        assert_eq!(mock.request_count(), 1, "4xx must not retry");
        assert_eq!(outcome.requests_made, 1);
    }

    #[test]
    fn test_retryable_errors_still_retry() {
        use crate::provider::mock::MockBackend;
        use crate::provider::ProviderError;

        let mock = Arc::new(MockBackend::new(vec![
            Err(ProviderError::RateLimited {
                retry_after: None,
                message: String::from("slow down"),
            }),
            Ok(String::from(
                r#"{"questions":[{"question":"Where does the citric acid cycle run?",
                                   "options":["Mitochondrial matrix","Cell nucleus","Ribosome","Golgi apparatus"],
                                   "answer":"Mitochondrial matrix",
                                   "explanation":"The cycle runs in the matrix."}]}"#,
            )),
        ]));
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
        assert_eq!(mock.request_count(), 2, "retryable errors must retry");
        assert_eq!(outcome.requests_made, 2);
        assert!(outcome.backend_error.is_none());
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
