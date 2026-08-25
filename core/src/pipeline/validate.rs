//! Deterministic post-generation quality gates. No LLM involvement: every
//! check is a pure function over the model output plus its source segment.
//! Invalid items are either repaired (normalization) or rejected so the
//! caller can retry the unit with a different seed.

use std::collections::HashSet;

/// Leading enumeration/bullet markers some models prepend to options.
const OPTION_PREFIXES: &[&str] = &["•", "-", "–", "*", "▪", "·"];

/// Strip `A)`, `(1)`, `3.`, `-`, `•` style prefixes and stray whitespace from
/// an answer-choice string.
pub fn normalize_choice(raw: &str) -> String {
    let mut text = raw.trim();
    loop {
        let stripped = strip_one_prefix(text);
        match stripped {
            Some(next) if next != text => text = next,
            _ => break,
        }
    }
    text.trim().to_string()
}

fn strip_one_prefix(text: &str) -> Option<&str> {
    let trimmed = text.trim_start();
    for marker in OPTION_PREFIXES {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return Some(rest.trim_start());
        }
    }

    // Letter prefix: `a)`, `B.`, `(c)`
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let second = bytes[1];
        let is_letter = first.is_ascii_alphabetic();
        let separator = matches!(second, b')' | b'.' | b':');
        if is_letter && separator {
            return Some(trimmed[2..].trim_start());
        }
        if first == b'(' {
            // `(a)` / `(1)` forms
            if bytes.len() >= 4 && bytes[2] == b')' && (bytes[1].is_ascii_alphabetic() || bytes[1].is_ascii_digit()) {
                return Some(trimmed[3..].trim_start());
            }
        }
    }

    // Number prefix: `1.` `12)` `7:`
    let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && digits < 4 {
        let after = &trimmed[digits..];
        if let Some(rest) = after.strip_prefix('.') .or_else(|| after.strip_prefix(')').or_else(|| after.strip_prefix(':'))) {
            return Some(rest.trim_start());
        }
    }
    None
}

/// Whether text points at figures/tables/media that are not part of the
/// extracted material (the classic hallucination vector in parsed PDFs).
pub fn references_missing_media(text: &str) -> bool {
    let lowered = text.to_lowercase();
    let patterns = [
        "figure ", "fig. ", "fig ", "diagram ", "chart above", "chart below",
        "table above", "table below", "image above", "image below",
        "shown above", "shown below", "pictured", "illustrated above",
        "illustrated below", "as seen in the image", "see appendix",
    ];
    patterns.iter().any(|pattern| lowered.contains(pattern))
}

/// Content words (lowercased, stopwords removed) used for grounding checks.
fn content_words(text: &str) -> HashSet<String> {
    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "and", "or", "but", "if", "then", "of", "to", "in", "on", "for",
        "with", "as", "by", "at", "from", "is", "are", "was", "were", "be", "been", "being",
        "it", "its", "this", "that", "these", "those", "which", "what", "who", "when",
        "where", "why", "how", "not", "no", "yes", "can", "could", "should", "would",
        "will", "shall", "may", "might", "must", "do", "does", "did", "have", "has", "had",
    ];
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| word.len() > 2)
        .map(str::to_lowercase)
        .filter(|word| !STOPWORDS.contains(&word.as_str()))
        .collect()
}

/// Fraction of an item's content words that appear in its source segment.
/// Low overlap strongly correlates with hallucinated questions.
pub fn grounding_ratio(item_text: &str, source_segment: &str) -> f32 {
    let item_words = content_words(item_text);
    if item_words.is_empty() {
        return 1.0;
    }
    let source_words = content_words(source_segment);
    let hits = item_words.iter().filter(|word| source_words.contains(*word)).count();
    hits as f32 / item_words.len() as f32
}

fn normalized_word_set(text: &str) -> HashSet<String> {
    content_words(text)
}

/// Jaccard similarity over normalized content words; cheap near-duplicate
/// detection across generated items.
pub fn similarity(left: &str, right: &str) -> f32 {
    let left_words = normalized_word_set(left);
    let right_words = normalized_word_set(right);
    if left_words.is_empty() || right_words.is_empty() {
        return 0.0;
    }
    let intersection = left_words.intersection(&right_words).count();
    let union = left_words.union(&right_words).count();
    intersection as f32 / union as f32
}

/// Outcome of validating one MCQ item.
#[derive(Debug)]
pub enum ItemVerdict {
    /// Normalized item, safe to persist.
    Accepted(serde_json::Value),
    /// Human-readable reason; triggers retry/drop upstream.
    Rejected(String),
}

/// Validate and normalize one MCQ item: exactly 4 unique non-empty choices,
/// the answer must equal one of them post-normalization, no figure
/// references, and grounded in its source segment.
pub fn validate_mcq_item(
    item: &serde_json::Value,
    source_segment: &str,
    min_grounding: f32,
) -> ItemVerdict {
    let question = match item.get("question").and_then(serde_json::Value::as_str) {
        Some(question) if !question.trim().is_empty() => question.trim(),
        _ => return ItemVerdict::Rejected(String::from("missing or empty question")),
    };

    let raw_options = match item.get("options").and_then(serde_json::Value::as_array) {
        Some(options) if options.len() == 4 => options,
        _ => return ItemVerdict::Rejected(format!("question {question:?}: expected exactly 4 options")),
    };

    let mut options = Vec::with_capacity(4);
    for raw in raw_options {
        let Some(option) = raw.as_str() else {
            return ItemVerdict::Rejected(format!("question {question:?}: non-string option"));
        };
        let normalized = normalize_choice(option);
        if normalized.is_empty() {
            return ItemVerdict::Rejected(format!("question {question:?}: empty option"));
        }
        options.push(normalized);
    }

    let lowered: HashSet<String> =
        options.iter().map(|option| option.to_lowercase()).collect();
    if lowered.len() != 4 {
        let duplicates: Vec<String> = options.clone();
        return ItemVerdict::Rejected(format!(
            "question {question:?}: duplicate choices {duplicates:?}"
        ));
    }

    let Some(answer) = item.get("answer").and_then(serde_json::Value::as_str) else {
        return ItemVerdict::Rejected(format!("question {question:?}: missing answer"));
    };
    let answer = normalize_choice(answer);
    if !options.contains(&answer) {
        return ItemVerdict::Rejected(format!(
            "question {question:?}: answer {answer:?} does not exactly match any option"
        ));
    }

    let explanation = item
        .get("explanation")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();

    let combined = format!("{question} {answer} {explanation}");
    if references_missing_media(&combined) {
        return ItemVerdict::Rejected(format!(
            "question {question:?}: references figures/media absent from the source"
        ));
    }

    let grounding = grounding_ratio(&combined, source_segment);
    if grounding < min_grounding {
        return ItemVerdict::Rejected(format!(
            "question {question:?}: grounding ratio {grounding:.2} below {min_grounding:.2}"
        ));
    }

    ItemVerdict::Accepted(serde_json::json!({
        "question": question,
        "options": options,
        "answer": answer,
        "explanation": explanation,
    }))
}

/// Validate and normalize one fill-in-the-blank item: sentence + answer +
/// hint present, answer appears (case-insensitively) in the source segment.
pub fn validate_completion_item(
    item: &serde_json::Value,
    source_segment: &str,
    min_grounding: f32,
) -> ItemVerdict {
    let Some(sentence) = item.get("sentence").and_then(serde_json::Value::as_str) else {
        return ItemVerdict::Rejected(String::from("missing sentence"));
    };
    let Some(answer) = item.get("answer").and_then(serde_json::Value::as_str) else {
        return ItemVerdict::Rejected(String::from("missing answer"));
    };

    let sentence = sentence.trim();
    let answer = answer.trim();
    if sentence.is_empty() || answer.is_empty() {
        return ItemVerdict::Rejected(String::from("empty sentence or answer"));
    }

    let source_lower = source_segment.to_lowercase();
    let answer_in_source = source_lower.contains(&answer.to_lowercase())
        || grounding_ratio(answer, source_segment) >= min_grounding;
    if !answer_in_source {
        return ItemVerdict::Rejected(format!(
            "completion answer {answer:?} not found in source segment"
        ));
    }

    let hint = item.get("hint").and_then(serde_json::Value::as_str).unwrap_or_default();
    let combined = format!("{sentence} {answer}");
    if references_missing_media(&combined) {
        return ItemVerdict::Rejected(String::from("references figures/media absent from the source"));
    }

    ItemVerdict::Accepted(serde_json::json!({
        "sentence": sentence,
        "answer": answer,
        "hint": hint,
    }))
}

/// Validate one essay item (lighter checks: structure + media references).
pub fn validate_essay_item(item: &serde_json::Value) -> ItemVerdict {
    let Some(question) = item.get("question").and_then(serde_json::Value::as_str) else {
        return ItemVerdict::Rejected(String::from("missing question"));
    };
    if question.trim().is_empty() {
        return ItemVerdict::Rejected(String::from("empty question"));
    }
    if references_missing_media(question) {
        return ItemVerdict::Rejected(String::from("references figures/media absent from the source"));
    }
    ItemVerdict::Accepted(item.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SOURCE: &str = "The mitochondrion is the powerhouse of the cell where respiration produces ATP through glycolysis and the citric acid cycle.";

    #[test]
    fn test_normalize_choice_strips_every_defect_format() {
        assert_eq!(normalize_choice("A) Mitochondria"), "Mitochondria");
        assert_eq!(normalize_choice("b. Ribosomes"), "Ribosomes");
        assert_eq!(normalize_choice("3. Golgi apparatus"), "Golgi apparatus");
        assert_eq!(normalize_choice("(d) Nucleus"), "Nucleus");
        assert_eq!(normalize_choice("- Chloroplast"), "Chloroplast");
        assert_eq!(normalize_choice("• Vacuole"), "Vacuole");
        assert_eq!(normalize_choice("  Plain option "), "Plain option");
    }

    #[test]
    fn test_duplicate_choices_rejected() {
        let item = json!({
            "question": "What is the powerhouse of the cell?",
            "options": ["Mitochondria", "mitochondria", "Ribosomes", "Nucleus"],
            "answer": "Mitochondria",
            "explanation": "Respiration happens there."
        });
        assert!(matches!(
            validate_mcq_item(&item, SOURCE, 0.15),
            ItemVerdict::Rejected(reason) if reason.contains("duplicate")
        ));
    }

    #[test]
    fn test_answer_not_among_options_rejected() {
        let item = json!({
            "question": "What is the powerhouse of the cell?",
            "options": ["Ribosomes", "Golgi apparatus", "Lysosome", "Nucleus"],
            "answer": "Mitochondria",
            "explanation": "It powers the cell."
        });
        assert!(matches!(
            validate_mcq_item(&item, SOURCE, 0.15),
            ItemVerdict::Rejected(reason) if reason.contains("does not exactly match")
        ));
    }

    #[test]
    fn test_figure_reference_rejected() {
        let item = json!({
            "question": "As shown in Figure 3, what does the diagram depict?",
            "options": ["Cell wall", "Mitochondria", "Ribosomes", "Nucleus"],
            "answer": "Mitochondria",
            "explanation": "The image above labels it."
        });
        assert!(matches!(
            validate_mcq_item(&item, SOURCE, 0.0),
            ItemVerdict::Rejected(reason) if reason.contains("figures")
        ));
    }

    #[test]
    fn test_ungrounded_question_rejected() {
        let item = json!({
            "question": "What is the capital city of France during winter sessions?",
            "options": ["Paris commune", "London borough", "Berlin district", "Madrid plaza"],
            "answer": "Paris commune",
            "explanation": "Geography trivia about european capitals."
        });
        assert!(matches!(
            validate_mcq_item(&item, SOURCE, 0.15),
            ItemVerdict::Rejected(reason) if reason.contains("grounding")
        ));
    }

    #[test]
    fn test_valid_mcq_is_normalized_and_accepted() {
        let item = json!({
            "question": "Which organelle is described as the powerhouse of the cell?",
            "options": ["A) Ribosome", "B) Mitochondria", "C) Nucleus", "D) Golgi"],
            "answer": "b) Mitochondria",
            "explanation": "Respiration producing ATP occurs in the mitochondria."
        });
        let verdict = validate_mcq_item(&item, SOURCE, 0.15);
        match verdict {
            ItemVerdict::Accepted(value) => {
                assert_eq!(value["options"][1], "Mitochondria");
                assert_eq!(value["answer"], "Mitochondria");
            }
            other => panic!("expected acceptance, got {other:?}"),
        }
    }

    #[test]
    fn test_completion_answer_must_exist_in_source() {
        let good = json!({
            "sentence": "The ____________ is the powerhouse of the cell.",
            "answer": "mitochondrion",
            "hint": "organelle"
        });
        assert!(matches!(
            validate_completion_item(&good, SOURCE, 0.5),
            ItemVerdict::Accepted(_)
        ));

        let bad = json!({
            "sentence": "The ____________ orbits the nucleus.",
            "answer": "electron cloud",
            "hint": "physics"
        });
        assert!(matches!(
            validate_completion_item(&bad, SOURCE, 0.5),
            ItemVerdict::Rejected(reason) if reason.contains("not found in source")
        ));
    }

    #[test]
    fn test_similarity_detects_near_duplicates() {
        let base = "What process converts light energy into chemical energy in plants?";
        let dupe = "What process converts light into chemical energy within plant cells?";
        let unrelated = "Explain the causes and effects of volcanic eruptions on climate.";
        assert!(similarity(base, dupe) > 0.5);
        assert!(similarity(base, unrelated) < 0.25);
    }
}
