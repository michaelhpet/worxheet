//! Structural quality gates; no LLM involved.

use std::collections::HashSet;

#[derive(Debug)]
pub enum ItemVerdict {
    Accepted(serde_json::Value),
    Rejected(String),
}

/// Exactly 4 unique non-empty choices; the answer must equal one of them.
pub fn validate_mcq_item(item: &serde_json::Value) -> ItemVerdict {
    let question = match item.get("question").and_then(serde_json::Value::as_str) {
        Some(question) if !question.trim().is_empty() => question.trim(),
        _ => return ItemVerdict::Rejected(String::from("missing or empty question")),
    };

    let raw_options = match item.get("options").and_then(serde_json::Value::as_array) {
        Some(options) if options.len() == 4 => options,
        _ => {
            return ItemVerdict::Rejected(format!(
                "question {question:?}: expected exactly 4 options"
            ))
        }
    };

    let mut options = Vec::with_capacity(4);
    for raw in raw_options {
        let Some(option) = raw.as_str().map(str::trim) else {
            return ItemVerdict::Rejected(format!("question {question:?}: non-string option"));
        };
        if option.is_empty() {
            return ItemVerdict::Rejected(format!("question {question:?}: empty option"));
        }
        options.push(option.to_string());
    }

    let lowered: HashSet<String> = options.iter().map(|option| option.to_lowercase()).collect();
    if lowered.len() != 4 {
        let duplicates: Vec<String> = options.clone();
        return ItemVerdict::Rejected(format!(
            "question {question:?}: duplicate choices {duplicates:?}"
        ));
    }

    let Some(answer) = item
        .get("answer")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|answer| !answer.is_empty())
    else {
        return ItemVerdict::Rejected(format!("question {question:?}: missing answer"));
    };
    if !options.iter().any(|option| option == answer) {
        return ItemVerdict::Rejected(format!(
            "question {question:?}: answer {answer:?} does not exactly match any option"
        ));
    }

    let explanation = item
        .get("explanation")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();

    ItemVerdict::Accepted(serde_json::json!({
        "question": question,
        "options": options,
        "answer": answer,
        "explanation": explanation,
    }))
}

/// Sentence + answer + hint present; answer appears verbatim in the source.
pub fn validate_completion_item(item: &serde_json::Value, source_segment: &str) -> ItemVerdict {
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

    if !source_segment
        .to_lowercase()
        .contains(&answer.to_lowercase())
    {
        return ItemVerdict::Rejected(format!(
            "completion answer {answer:?} not found in source segment"
        ));
    }

    let hint = item
        .get("hint")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();

    ItemVerdict::Accepted(serde_json::json!({
        "sentence": sentence,
        "answer": answer,
        "hint": hint,
    }))
}

pub fn validate_essay_item(item: &serde_json::Value) -> ItemVerdict {
    let Some(question) = item.get("question").and_then(serde_json::Value::as_str) else {
        return ItemVerdict::Rejected(String::from("missing question"));
    };
    if question.trim().is_empty() {
        return ItemVerdict::Rejected(String::from("empty question"));
    }
    ItemVerdict::Accepted(item.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SOURCE: &str = "The mitochondrion is the powerhouse of the cell where respiration produces ATP through glycolysis and the citric acid cycle.";

    #[test]
    fn test_duplicate_choices_rejected() {
        let item = json!({
            "question": "What is the powerhouse of the cell?",
            "options": ["Mitochondria", "mitochondria", "Ribosomes", "Nucleus"],
            "answer": "Mitochondria",
            "explanation": "Respiration happens there."
        });
        assert!(matches!(
            validate_mcq_item(&item),
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
            validate_mcq_item(&item),
            ItemVerdict::Rejected(reason) if reason.contains("does not exactly match")
        ));
    }

    #[test]
    fn test_prefixed_options_rejected() {
        let item = json!({
            "question": "Which organelle is described as the powerhouse of the cell?",
            "options": ["A) Ribosome", "B) Mitochondria", "C) Nucleus", "D) Golgi"],
            "answer": "b) Mitochondria",
            "explanation": "Respiration producing ATP occurs in the mitochondria."
        });
        assert!(matches!(
            validate_mcq_item(&item),
            ItemVerdict::Rejected(reason) if reason.contains("does not exactly match")
        ));
    }

    #[test]
    fn test_valid_mcq_accepted() {
        let item = json!({
            "question": "Which organelle is described as the powerhouse of the cell?",
            "options": ["Ribosome", "Mitochondria", "Nucleus", "Golgi"],
            "answer": "Mitochondria",
            "explanation": "Respiration producing ATP occurs in the mitochondria."
        });
        assert!(matches!(validate_mcq_item(&item), ItemVerdict::Accepted(_)));
    }

    #[test]
    fn test_completion_answer_must_exist_in_source() {
        let good = json!({
            "sentence": "The ____________ is the powerhouse of the cell.",
            "answer": "mitochondrion",
            "hint": "organelle"
        });
        assert!(matches!(
            validate_completion_item(&good, SOURCE),
            ItemVerdict::Accepted(_)
        ));

        let bad = json!({
            "sentence": "The ____________ orbits the nucleus.",
            "answer": "electron cloud",
            "hint": "physics"
        });
        assert!(matches!(
            validate_completion_item(&bad, SOURCE),
            ItemVerdict::Rejected(reason) if reason.contains("not found in source")
        ));
    }
}
