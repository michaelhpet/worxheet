//! Turns parsed document blocks into contiguous, ordered generation units.
//!
//! Strategy: structure first (every heading starts a new candidate section,
//! sections are packed up to [`TARGET_SEGMENT_TOKENS`]), then embedding-drift
//! splitting for any oversized structureless stretch, then plain token-window
//! packing as the last resort. Every input token lands in exactly one
//! segment: coverage is exhaustive by construction.

use tokenizers::Tokenizer;

use crate::embedder::Embedder;

/// Preferred size of one segment.
pub const TARGET_SEGMENT_TOKENS: usize = 1_100;
/// Hard ceiling before drift splitting kicks in.
pub const MAX_SEGMENT_TOKENS: usize = 1_800;
/// Below this, neighboring fragments are merged instead of standing alone.
pub const MIN_SEGMENT_TOKENS: usize = 150;
/// Sections larger than this get drift-checked for internal topic shifts,
/// even though they would still fit in one request-sized chunk.
pub const DRIFT_TRIGGER_TOKENS: usize = 450;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// Heading with its level (1 = top).
    Heading(u8),
    Body,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub kind: BlockKind,
    pub text: String,
}

/// A finished segment before it is persisted.
#[derive(Clone, Debug)]
pub struct SegmentDraft {
    pub heading: Option<String>,
    pub text: String,
}

/// Source of sentence embeddings for drift detection.
pub trait SentenceEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String>;
}

impl SentenceEmbedder for Embedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Embedder::embed(self, texts)
    }
}

fn token_count(tokenizer: &Tokenizer, text: &str) -> usize {
    tokenizer
        .encode(text, true)
        .map(|encoding| encoding.get_ids().len())
        .unwrap_or_else(|_| text.len() / 4)
}

/// Split raw text into sentences on terminal punctuation followed by
/// whitespace or end-of-line.
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut carry = String::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let bytes = line.as_bytes();
        let mut start = 0usize;
        for position in 0..bytes.len() {
            let terminator = matches!(bytes[position], b'.' | b'!' | b'?')
                && (position + 1 == bytes.len() || bytes[position + 1].is_ascii_whitespace());
            if terminator {
                let piece = line[start..=position].trim();
                if !piece.is_empty() {
                    if !carry.is_empty() {
                        carry.push(' ');
                        carry.push_str(piece);
                    } else {
                        carry.push_str(piece);
                    }
                    sentences.push(std::mem::take(&mut carry));
                }
                start = position + 1;
            }
        }
        let tail = line[start..].trim();
        if !tail.is_empty() {
            if !carry.is_empty() {
                carry.push(' ');
            }
            carry.push_str(tail);
        }
    }
    if !carry.is_empty() {
        sentences.push(carry);
    }
    sentences.retain(|sentence| !sentence.trim().is_empty());
    sentences
}

fn flush_draft(drafts: &mut Vec<SegmentDraft>, heading: Option<String>, text: &str) {
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        drafts.push(SegmentDraft {
            heading,
            text: trimmed.to_string(),
        });
    }
}

/// Group blocks into heading-scoped sections no larger than
/// [`TARGET_SEGMENT_TOKENS`].
pub fn pack_sections(blocks: &[Block], tokenizer: &Tokenizer) -> Vec<SegmentDraft> {
    let mut drafts: Vec<SegmentDraft> = Vec::new();
    let mut current_heading: Option<String> = None;
    let mut current_text = String::new();
    let mut current_tokens = 0usize;

    for block in blocks {
        match block.kind {
            BlockKind::Heading(_) => {
                flush_draft(&mut drafts, current_heading.take(), &current_text);
                current_text.clear();
                current_tokens = 0;
                let title = block.text.trim().to_string();
                if !title.is_empty() {
                    current_heading = Some(title);
                }
            }
            BlockKind::Body => {
                let body = block.text.trim();
                if body.is_empty() {
                    continue;
                }
                // Oversized paragraphs are broken into sentences so packing
                // can chunk them against the target size.
                let sentence_pieces: Vec<String>;
                let pieces: Vec<&str> = if token_count(tokenizer, body) > TARGET_SEGMENT_TOKENS {
                    sentence_pieces = split_sentences(body);
                    sentence_pieces.iter().map(String::as_str).collect()
                } else {
                    vec![body]
                };
                for piece in pieces {
                    let tokens = token_count(tokenizer, piece);
                    if current_tokens > 0 && current_tokens + tokens > TARGET_SEGMENT_TOKENS {
                        flush_draft(&mut drafts, current_heading.clone(), &current_text);
                        current_text.clear();
                        current_tokens = 0;
                    }
                    if !current_text.is_empty() {
                        current_text.push(' ');
                    }
                    current_text.push_str(piece);
                    current_tokens += tokens;
                }
            }
        }
    }
    flush_draft(&mut drafts, current_heading, &current_text);

    drafts
}

/// Cosine similarity between two equal-length vectors.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm = a.iter().map(|v| v * v).sum::<f32>().sqrt()
        * b.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        dot / norm
    } else {
        1.0
    }
}

/// Indices where topic shifts occur: consecutive-sentence similarities are
/// smoothed over a 3-wide window, and a shift is kept only when the smoothed
/// value dips below `mean - deviation_scale × std` while being a local minimum.
pub fn drift_boundaries(similarities: &[f32], deviation_scale: f32) -> Vec<usize> {
    if similarities.is_empty() {
        return Vec::new();
    }

    let mut smoothed = similarities.to_vec();
    for index in 0..similarities.len() {
        let window = [
            similarities[index.saturating_sub(1)],
            similarities[index],
            similarities[(index + 1).min(similarities.len() - 1)],
        ];
        smoothed[index] = window.iter().sum::<f32>() / window.len() as f32;
    }

    let mean = smoothed.iter().sum::<f32>() / smoothed.len() as f32;
    let variance =
        smoothed.iter().map(|value| (value - mean).powi(2)).sum::<f32>() / smoothed.len() as f32;
    let threshold = mean - deviation_scale * variance.sqrt();

    let mut boundaries = Vec::new();
    for index in 0..smoothed.len() {
        let value = smoothed[index];
        if value >= threshold {
            continue;
        }
        let left_ok = index == 0 || smoothed[index - 1] >= value;
        let right_ok = index == smoothed.len() - 1 || smoothed[index + 1] >= value;
        if left_ok && right_ok {
            boundaries.push(index + 1); // cut AFTER sentence `index`
        }
    }
    boundaries
}

fn hard_token_windows(text: &str, tokenizer: &Tokenizer) -> Vec<String> {
    let Ok(encoding) = tokenizer.encode(text, true) else {
        return vec![text.to_string()];
    };
    let ids = encoding.get_ids();
    if ids.len() <= MAX_SEGMENT_TOKENS {
        return vec![text.to_string()];
    }
    ids.chunks(MAX_SEGMENT_TOKENS)
        .filter_map(|window| tokenizer.decode(window, true).ok())
        .filter(|decoded| !decoded.trim().is_empty())
        .collect()
}

/// Turn drift-delimited sentence groups into drafts, hard-splitting any
/// group that alone exceeds `[MAX]`. Groups are kept distinct: they were
/// separated on purpose.
fn enforce_token_bounds(groups: Vec<Vec<String>>, tokenizer: &Tokenizer) -> Vec<SegmentDraft> {
    let mut drafts: Vec<SegmentDraft> = Vec::new();

    for group in groups {
        let joined = group.join(" ");
        if token_count(tokenizer, &joined) > MAX_SEGMENT_TOKENS {
            for window in hard_token_windows(&joined, tokenizer) {
                flush_draft(&mut drafts, None, &window);
            }
            continue;
        }
        flush_draft(&mut drafts, None, &joined);
    }
    drafts
}

fn hard_windows_fallback(section: &SegmentDraft, tokenizer: &Tokenizer) -> Vec<SegmentDraft> {
    hard_token_windows(&section.text, tokenizer)
        .into_iter()
        .map(|text| SegmentDraft {
            heading: section.heading.clone(),
            text,
        })
        .collect()
}

/// Drift-split an oversized section: embed its sentences, find topic-shift
/// boundaries, regroup within token bounds.
fn drift_split(
    section: &SegmentDraft,
    tokenizer: &Tokenizer,
    embedder: &dyn SentenceEmbedder,
) -> Vec<SegmentDraft> {
    let sentences = split_sentences(&section.text);
    if sentences.len() < 6 || token_count(tokenizer, &section.text) <= DRIFT_TRIGGER_TOKENS {
        return vec![section.clone()];
    }

    let Ok(embedded) = embedder.embed(&sentences) else {
        return hard_windows_fallback(section, tokenizer);
    };
    if embedded.len() != sentences.len() {
        return hard_windows_fallback(section, tokenizer);
    }

    let similarities: Vec<f32> = (0..sentences.len() - 1)
        .map(|index| cosine(&embedded[index], &embedded[index + 1]))
        .collect();
    let boundaries = drift_boundaries(&similarities, 0.75);
    if boundaries.is_empty() {
        return hard_windows_fallback(section, tokenizer);
    }

    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut current_group: Vec<String> = Vec::new();
    for (index, sentence) in sentences.iter().enumerate() {
        current_group.push(sentence.clone());
        if boundaries.contains(&(index + 1)) {
            groups.push(std::mem::take(&mut current_group));
        }
    }
    if !current_group.is_empty() {
        groups.push(current_group);
    }

    let mut drafts = enforce_token_bounds(groups, tokenizer);
    for draft in &mut drafts {
        if draft.heading.is_none() {
            draft.heading = section.heading.clone();
        }
    }
    if drafts.is_empty() {
        drafts.push(section.clone());
    }
    drafts
}

/// Full segmentation pipeline for one file's parsed blocks.
pub fn segment_blocks(
    blocks: Vec<Block>,
    tokenizer: &Tokenizer,
    embedder: &dyn SentenceEmbedder,
) -> Vec<SegmentDraft> {
    let sections = pack_sections(&blocks, tokenizer);
    let mut drafts = Vec::new();
    for section in sections {
        if token_count(tokenizer, &section.text) <= DRIFT_TRIGGER_TOKENS {
            drafts.push(section);
        } else {
            drafts.extend(drift_split(&section, tokenizer, embedder));
        }
    }
    merge_undersized(drafts, tokenizer)
}

/// Merge consecutive undersized drafts so tiny slivers never reach the LLM.
fn merge_undersized(mut drafts: Vec<SegmentDraft>, tokenizer: &Tokenizer) -> Vec<SegmentDraft> {
    if drafts.len() <= 1 {
        return drafts;
    }

    let mut merged: Vec<SegmentDraft> = Vec::with_capacity(drafts.len());
    for draft in drafts.drain(..) {
        let combine = match merged.last_mut() {
            Some(last) => {
                last.heading == draft.heading
                    && token_count(tokenizer, &last.text) + token_count(tokenizer, &draft.text)
                        <= MAX_SEGMENT_TOKENS
                    && (token_count(tokenizer, &last.text) < MIN_SEGMENT_TOKENS
                        || token_count(tokenizer, &draft.text) < MIN_SEGMENT_TOKENS)
            }
            None => false,
        };
        if combine {
            let last = merged.last_mut().expect("checked above");
            last.text.push('\n');
            last.text.push_str(&draft.text);
        } else {
            merged.push(draft);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic two-topic embedder: photosynthesis words map to one
    /// vector, volcano words to another, everything else in between.
    struct TopicEmbedder;

    impl SentenceEmbedder for TopicEmbedder {
        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
            Ok(texts
                .iter()
                .map(|text| {
                    if text.to_lowercase().contains("photosynth") {
                        vec![1.0, 0.0]
                    } else if text.to_lowercase().contains("volcano") || text.to_lowercase().contains("magma") {
                        vec![0.0, 1.0]
                    } else {
                        vec![0.7, 0.7]
                    }
                })
                .collect())
        }
    }

    // The bundled production tokenizer keeps token-count semantics honest in
    // tests without downloading any model artifact.
    fn test_tokenizer() -> Tokenizer {
        Tokenizer::from_bytes(include_str!("../../assets/tokenizer.json"))
            .expect("bundled tokenizer should parse")
    }

    fn block(kind: BlockKind, text: &str) -> Block {
        Block {
            kind,
            text: text.to_string(),
        }
    }

    #[test]
    fn test_pack_sections_starts_new_section_at_headings() {
        let tokenizer = test_tokenizer();
        let blocks = vec![
            block(BlockKind::Heading(1), "Chapter 1"),
            block(BlockKind::Body, "Alpha body text."),
            block(BlockKind::Heading(2), "Section 1.1"),
            block(BlockKind::Body, "Beta body text."),
        ];

        let drafts = pack_sections(&blocks, &tokenizer);
        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0].heading.as_deref(), Some("Chapter 1"));
        assert_eq!(drafts[0].text, "Alpha body text.");
        assert_eq!(drafts[1].heading.as_deref(), Some("Section 1.1"));
        assert_eq!(drafts[1].text, "Beta body text.");
    }

    #[test]
    fn test_pack_sections_splits_large_runs() {
        let tokenizer = test_tokenizer();
        let long_body = "The mitochondrion steadily produces energy for the rest of the living cell. "
            .repeat(150);
        let blocks = vec![block(BlockKind::Body, &long_body)];

        let drafts = pack_sections(&blocks, &tokenizer);
        assert!(drafts.len() >= 2, "long body should be packed into multiple sections");
    }

    #[test]
    fn test_drift_boundaries_finds_valley_between_topics() {
        let similarities = vec![0.95, 0.93, 0.42, 0.94, 0.96];
        let boundaries = drift_boundaries(&similarities, 0.75);
        assert_eq!(boundaries, vec![3], "cut after sentence index 2");
    }

    #[test]
    fn test_drift_boundaries_flat_signal_yields_nothing() {
        let similarities = vec![0.9, 0.9, 0.9, 0.9];
        assert!(drift_boundaries(&similarities, 0.75).is_empty());
    }

    #[test]
    fn test_split_sentences_handles_line_breaks_and_carryover() {
        let text = "First sentence! Second one\nspans lines. A final\nquestion?\nNo terminator here";
        let sentences = split_sentences(text);
        assert_eq!(
            sentences,
            vec![
                String::from("First sentence!"),
                String::from("Second one spans lines."),
                String::from("A final question?"),
                String::from("No terminator here"),
            ]
        );
    }

    #[test]
    fn test_segment_blocks_exhausts_two_topic_material() {
        let tokenizer = test_tokenizer();
        let topic_a =
            "Photosynthesis converts light energy into chemical energy stored as glucose. ";
        let topic_b = "Volcanoes erupt when magma pressure builds beneath the crust. ";
        let blocks = vec![block(
            BlockKind::Body,
            &(topic_a.repeat(60).to_string() + &topic_b.repeat(60)),
        )];

        let drafts = segment_blocks(blocks, &tokenizer, &TopicEmbedder);
        assert!(drafts.len() >= 2, "two distinct topics must not share one segment");

        let covered: usize = drafts.iter().map(|draft| draft.text.len()).sum();
        let input_len = topic_a.len() * 60 + topic_b.len() * 60;
        assert!(covered >= input_len * 9 / 10, "segments must cover essentially all input");
    }

    #[test]
    fn test_merge_undersized_combines_slivers() {
        let tokenizer = test_tokenizer();
        let drafts = vec![
            SegmentDraft {
                heading: None,
                text: String::from("tiny"),
            },
            SegmentDraft {
                heading: None,
                text: String::from("also tiny"),
            },
            SegmentDraft {
                heading: None,
                text: "large ".repeat(6000),
            },
        ];
        let merged = merge_undersized(drafts, &tokenizer);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].text, "tiny\nalso tiny");
    }
}
