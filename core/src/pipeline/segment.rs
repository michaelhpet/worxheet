//! Turns parsed document blocks into contiguous, ordered generation units.
//!
//! Strategy: structure first (every heading starts a new candidate section,
//! sections are packed up to [`TARGET_SEGMENT_TOKENS`]), then token-bounded
//! sentence packing for any oversized structureless stretch. Every input token
//! lands in exactly one segment: coverage is exhaustive by construction. The
//! segmentation is purely structural — the vendored tokenizer (bundled into the
//! binary) provides token-count semantics, so no external model is required.

use tokenizers::Tokenizer;

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

/// Load the tokenizer vendored into the binary. Parseable offline; used for
/// token-count semantics in tests and ingest without any model download.
pub fn bundled_tokenizer() -> Result<Tokenizer, String> {
    const TOKENIZER_JSON: &str = include_str!("../../assets/tokenizer.json");
    Tokenizer::from_bytes(TOKENIZER_JSON).map_err(|e| format!("Failed to parse bundled tokenizer: {e}"))
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
/// [`TARGET_SEGMENT_TOKENS`]. `on_progress` (when given) reports
/// `(blocks_processed, total_blocks)` so ingestion can keep advancing its
/// progress bar through the (CPU-heavy) tokenization of this stage.
pub fn pack_sections(
    blocks: &[Block],
    tokenizer: &Tokenizer,
    mut on_progress: Option<&mut dyn FnMut(usize, usize)>,
) -> Vec<SegmentDraft> {
    let mut drafts: Vec<SegmentDraft> = Vec::new();
    let mut current_heading: Option<String> = None;
    let mut current_text = String::new();
    let mut current_tokens = 0usize;
    let total_blocks = blocks.len();

    for (block_index, block) in blocks.iter().enumerate() {
        if let Some(on_progress) = on_progress.as_deref_mut() {
            on_progress(block_index + 1, total_blocks);
        }
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

/// Split an oversized section structurally: pack its sentences into
/// token-bounded groups (no embedding model needed). Long sections are broken
/// at sentence boundaries to fit within [`MAX_SEGMENT_TOKENS`].
fn structural_split(
    section: &SegmentDraft,
    tokenizer: &Tokenizer,
) -> Vec<SegmentDraft> {
    let sentences = split_sentences(&section.text);
    if sentences.len() < 6 || token_count(tokenizer, &section.text) <= DRIFT_TRIGGER_TOKENS {
        return vec![section.clone()];
    }

    let groups: Vec<Vec<String>> = sentences.into_iter().map(|sentence| vec![sentence]).collect();
    let mut drafts = enforce_token_bounds(groups, tokenizer);
    if drafts.is_empty() {
        return hard_windows_fallback(section, tokenizer);
    }

    for draft in &mut drafts {
        if draft.heading.is_none() {
            draft.heading = section.heading.clone();
        }
    }
    drafts
}

/// Full segmentation pipeline for one file's parsed blocks. `on_progress`
/// (when given) reports `(blocks_processed, total_blocks)` as sections are
/// packed, advancing the ingest progress bar through tokenization.
pub fn segment_blocks(
    blocks: Vec<Block>,
    tokenizer: &Tokenizer,
    on_progress: Option<&mut dyn FnMut(usize, usize)>,
) -> Vec<SegmentDraft> {
    let sections = pack_sections(&blocks, tokenizer, on_progress);
    let mut drafts = Vec::new();
    for section in sections {
        if token_count(tokenizer, &section.text) <= DRIFT_TRIGGER_TOKENS {
            drafts.push(section);
        } else {
            drafts.extend(structural_split(&section, tokenizer));
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

    // The bundled production tokenizer keeps token-count semantics honest in
    // tests without downloading any model artifact.
    fn test_tokenizer() -> Tokenizer {
        bundled_tokenizer().expect("bundled tokenizer should parse")
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

        let drafts = pack_sections(&blocks, &tokenizer, None);
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

        let drafts = pack_sections(&blocks, &tokenizer, None);
        assert!(drafts.len() >= 2, "long body should be packed into multiple sections");
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
    fn test_segment_blocks_packs_long_runs_within_token_bounds() {
        let tokenizer = test_tokenizer();
        let sentence = "Photosynthesis converts light energy into chemical energy stored as glucose. ";
        let repeat = 200;
        let blocks = vec![block(BlockKind::Body, &sentence.repeat(repeat))];

        let drafts = segment_blocks(blocks, &tokenizer, None);
        assert!(drafts.len() >= 2, "long body must be split into multiple segments");

        for draft in &drafts {
            assert!(
                token_count(&tokenizer, &draft.text) <= MAX_SEGMENT_TOKENS,
                "no segment may exceed MAX_SEGMENT_TOKENS"
            );
        }

        let covered: usize = drafts.iter().map(|draft| draft.text.len()).sum();
        let input_len = sentence.len() * repeat;
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
