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
/// Hard ceiling before splitting kicks in.
pub const MAX_SEGMENT_TOKENS: usize = 1_800;
/// Below this, neighboring fragments are merged instead of standing alone.
pub const MIN_SEGMENT_TOKENS: usize = 150;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// Heading with its level (1 = top).
    Heading(u8),
    Body,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub text: String,
}

/// A finished segment before it is persisted.
#[derive(Clone, Debug)]
pub struct SegmentDraft {
    pub heading: Option<String>,
    pub text: String,
    pub tokens: usize,
}

/// Load the tokenizer vendored into the binary. Parseable offline; used for
/// token-count semantics in tests and ingest without any model download.
pub fn bundled_tokenizer() -> Result<Tokenizer, String> {
    const TOKENIZER_JSON: &str = include_str!("../../assets/tokenizer.json");
    Tokenizer::from_bytes(TOKENIZER_JSON)
        .map_err(|e| format!("Failed to parse bundled tokenizer: {e}"))
}

/// In-memory tokenization cost tally, accumulated during one file's
/// segmentation and flushed to a log afterwards, so the hot tokenization loop
/// never performs disk I/O.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokenizeTrace {
    pub calls: usize,
    pub tokens: usize,
}

fn token_count(tokenizer: &Tokenizer, text: &str, trace: Option<&mut TokenizeTrace>) -> usize {
    let tokens = tokenizer
        .encode_fast(text, true)
        .map(|encoding| encoding.get_ids().len())
        .unwrap_or_else(|_| text.len() / 4);
    if let Some(trace) = trace {
        trace.calls += 1;
        trace.tokens += tokens;
    }
    tokens
}

/// Parallel precompute of per-block token counts. The tokenizer is the
/// dominant cost of packing, so bodies are encoded in parallel (disjoint
/// blocks per thread); the caller-visible [`TokenizeTrace`] is never touched
/// from worker threads — each returns a local tally that is summed afterwards.
fn parallel_body_counts(blocks: &[Block], tokenizer: &Tokenizer) -> (Vec<usize>, TokenizeTrace) {
    let body_indices: Vec<usize> = blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| matches!(block.kind, BlockKind::Body))
        .map(|(i, _)| i)
        .collect();
    if body_indices.is_empty() {
        return (vec![0usize; blocks.len()], TokenizeTrace::default());
    }

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .max(1);
    let chunk_size = body_indices.len().div_ceil(threads).max(1);

    let (counts, tallies) = std::thread::scope(|s| {
        let mut handles = Vec::with_capacity(threads.min(body_indices.len()));
        for chunk in body_indices.chunks(chunk_size) {
            let chunk = chunk.to_vec();
            handles.push(s.spawn(move || {
                let mut results = Vec::with_capacity(chunk.len());
                let mut tally = TokenizeTrace::default();
                for &index in &chunk {
                    let count = token_count(tokenizer, blocks[index].text.trim(), None);
                    results.push((index, count));
                    tally.calls += 1;
                    tally.tokens += count;
                }
                (results, tally)
            }));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("token counting task panicked"))
            .fold(
                (vec![0usize; blocks.len()], TokenizeTrace::default()),
                |(mut counts, mut total), (results, tally)| {
                    for (index, count) in results {
                        counts[index] = count;
                    }
                    total.calls += tally.calls;
                    total.tokens += tally.tokens;
                    (counts, total)
                },
            )
    });
    (counts, tallies)
}

/// Split raw text into sentences on terminal punctuation followed by
/// whitespace or end-of-line. Decimal points ("3.14"), ellipses ("..." /
/// "…"), and common abbreviations ("Dr.", "e.g.", "U.S.") never terminate a
/// sentence.
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut carry = String::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let mut start = 0usize;
        for (position, ch) in line.char_indices() {
            let is_terminal = matches!(ch, '.' | '!' | '?')
                && line[position + ch.len_utf8()..]
                    .chars()
                    .next()
                    .is_none_or(|c| c.is_ascii_whitespace());
            // Don't split on decimal points: "3.14" → keep intact
            let prev_char = line[..position].chars().next_back();
            let next_char = line[position + ch.len_utf8()..].chars().next();
            let is_decimal = ch == '.'
                && prev_char.is_some_and(|c| c.is_ascii_digit())
                && next_char.is_some_and(|c| c.is_ascii_digit());
            // Don't split on ellipses: "..." or "…" → keep intact
            let is_ellipsis = ch == '.'
                && (prev_char == Some('.')
                    || next_char == Some('.')
                    || prev_char == Some('…')
                    || next_char == Some('…'));
            // Don't split on common abbreviations: "e.g.", "Dr.", "vs." → keep intact
            let is_abbreviation = ch == '.' && is_known_abbreviation(&line[..=position]);
            if is_terminal && !is_decimal && !is_ellipsis && !is_abbreviation {
                let piece = line[start..=position + ch.len_utf8() - 1].trim();
                if !piece.is_empty() {
                    if !carry.is_empty() {
                        carry.push(' ');
                        carry.push_str(piece);
                    } else {
                        carry.push_str(piece);
                    }
                    sentences.push(std::mem::take(&mut carry));
                }
                start = position + ch.len_utf8();
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

/// True if the text ending at the given trailing period is a known
/// abbreviation that must not terminate a sentence ("e.g.", "Dr.", "U.S.").
fn is_known_abbreviation(token_end: &str) -> bool {
    const ABBREVIATIONS: &[&str] = &[
        "a.m.", "dr.", "e.g.", "etc.", "i.e.", "mr.", "mrs.", "ms.", "no.", "p.m.", "prof.",
        "u.s.a.", "u.s.", "vs.",
    ];
    let bytes = token_end.as_bytes();
    if bytes.last() != Some(&b'.') {
        return false;
    }
    // Walk back over letters and inner dots to capture the whole token
    // ("e.g.", "U.S."), stopping at any other separator.
    let mut start = bytes.len() - 1;
    while start > 0 {
        let b = bytes[start - 1];
        if b.is_ascii_alphanumeric() || b == b'.' {
            start -= 1;
        } else {
            break;
        }
    }
    let candidate = token_end[start..].to_ascii_lowercase();
    ABBREVIATIONS.contains(&candidate.as_str())
}

fn flush_draft(drafts: &mut Vec<SegmentDraft>, heading: Option<String>, text: &str, tokens: usize) {
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        drafts.push(SegmentDraft {
            heading,
            text: trimmed.to_string(),
            tokens,
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
    mut trace: Option<&mut TokenizeTrace>,
) -> Vec<SegmentDraft> {
    let mut drafts: Vec<SegmentDraft> = Vec::new();
    let mut current_heading: Option<String> = None;
    let mut current_text = String::new();
    let mut current_tokens = 0usize;
    let total_blocks = blocks.len();

    // Body token counts are the bulk of packing cost; encode them once, in
    // parallel, then reuse the cached counts for the oversized check and for
    // single-piece bodies (killing the previous double tokenization).
    let (block_tokens, parallel_trace) = parallel_body_counts(blocks, tokenizer);
    if let Some(trace) = trace.as_deref_mut() {
        trace.calls += parallel_trace.calls;
        trace.tokens += parallel_trace.tokens;
    }

    for (block_index, block) in blocks.iter().enumerate() {
        if let Some(on_progress) = on_progress.as_deref_mut() {
            on_progress(block_index + 1, total_blocks);
        }
        match block.kind {
            BlockKind::Heading(_) => {
                flush_draft(
                    &mut drafts,
                    current_heading.take(),
                    &current_text,
                    current_tokens,
                );
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
                let body_tokens = block_tokens[block_index];
                // Oversized paragraphs are broken into sentences so packing
                // can chunk them against the target size.
                let sentence_pieces: Vec<String>;
                let pieces: Vec<&str> = if body_tokens > TARGET_SEGMENT_TOKENS {
                    sentence_pieces = split_sentences(body);
                    sentence_pieces.iter().map(String::as_str).collect()
                } else {
                    vec![body]
                };
                for piece in &pieces {
                    // Single-piece bodies reuse the precomputed parallel count;
                    // oversized bodies tokenize each sentence piece as before.
                    let tokens = if pieces.len() == 1 {
                        body_tokens
                    } else {
                        token_count(tokenizer, piece, trace.as_deref_mut())
                    };
                    if current_tokens > 0 && current_tokens + tokens > TARGET_SEGMENT_TOKENS {
                        flush_draft(
                            &mut drafts,
                            current_heading.clone(),
                            &current_text,
                            current_tokens,
                        );
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
    flush_draft(&mut drafts, current_heading, &current_text, current_tokens);

    drafts
}

fn hard_token_windows(
    text: &str,
    tokenizer: &Tokenizer,
    trace: Option<&mut TokenizeTrace>,
) -> Vec<(String, usize)> {
    let Ok(encoding) = tokenizer.encode_fast(text, true) else {
        return vec![(text.to_string(), text.len() / 4)];
    };
    if let Some(trace) = trace {
        trace.calls += 1;
        trace.tokens += encoding.get_ids().len();
    }
    let ids = encoding.get_ids();
    if ids.len() <= MAX_SEGMENT_TOKENS {
        return vec![(text.to_string(), ids.len())];
    }
    ids.chunks(MAX_SEGMENT_TOKENS)
        .filter_map(|window| {
            tokenizer
                .decode(window, true)
                .ok()
                .map(|d| (d, window.len()))
        })
        .filter(|(decoded, _)| !decoded.trim().is_empty())
        .collect()
}

fn hard_windows_fallback(
    section: &SegmentDraft,
    tokenizer: &Tokenizer,
    trace: Option<&mut TokenizeTrace>,
) -> Vec<SegmentDraft> {
    hard_token_windows(&section.text, tokenizer, trace)
        .into_iter()
        .map(|(text, tokens)| SegmentDraft {
            heading: section.heading.clone(),
            text,
            tokens,
        })
        .collect()
}

/// Full segmentation pipeline for one file's parsed blocks. `on_progress`
/// (when given) reports `(blocks_processed, total_blocks)` as sections are
/// packed, advancing the ingest progress bar through tokenization. `trace`
/// (when given) accumulates every tokenization call so callers can log the
/// cost once per file.
pub fn segment_blocks(
    blocks: Vec<Block>,
    tokenizer: &Tokenizer,
    on_progress: Option<&mut dyn FnMut(usize, usize)>,
    mut trace: Option<&mut TokenizeTrace>,
) -> Vec<SegmentDraft> {
    let sections = pack_sections(&blocks, tokenizer, on_progress, trace.as_deref_mut());
    let mut drafts = Vec::with_capacity(sections.len());
    for section in sections {
        if section.tokens <= MAX_SEGMENT_TOKENS {
            drafts.push(section);
        } else {
            drafts.extend(hard_windows_fallback(
                &section,
                tokenizer,
                trace.as_deref_mut(),
            ));
        }
    }
    merge_undersized(drafts, tokenizer)
}

/// Merge consecutive undersized drafts so tiny slivers never reach the LLM.
/// Heading boundaries are respected only as a tie-breaker preference; any
/// sub-`MIN_SEGMENT_TOKENS` fragment merges into a neighbor regardless of
/// heading, otherwise a marker-like sliver between differently-headed sections
/// (e.g. a bare page marker after a heading bump) survives as a standalone
/// content-less segment.
fn merge_undersized(mut drafts: Vec<SegmentDraft>, _tokenizer: &Tokenizer) -> Vec<SegmentDraft> {
    if drafts.len() <= 1 {
        return drafts;
    }

    let mut merged: Vec<SegmentDraft> = Vec::with_capacity(drafts.len());
    for draft in drafts.drain(..) {
        let combine = match merged.last() {
            Some(last) => {
                last.tokens + draft.tokens <= MAX_SEGMENT_TOKENS
                    && (last.tokens < MIN_SEGMENT_TOKENS || draft.tokens < MIN_SEGMENT_TOKENS)
            }
            None => false,
        };
        if combine {
            let last = merged.last_mut().expect("checked above");
            last.text.push('\n');
            last.text.push_str(&draft.text);
            last.heading = match (&last.heading, &draft.heading) {
                (None, Some(ref heading)) => Some(heading.clone()),
                _ => last.heading.clone(),
            };
            last.tokens += draft.tokens;
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

        let drafts = pack_sections(&blocks, &tokenizer, None, None);
        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0].heading.as_deref(), Some("Chapter 1"));
        assert_eq!(drafts[0].text, "Alpha body text.");
        assert_eq!(drafts[1].heading.as_deref(), Some("Section 1.1"));
        assert_eq!(drafts[1].text, "Beta body text.");
    }

    #[test]
    fn test_pack_sections_splits_large_runs() {
        let tokenizer = test_tokenizer();
        let long_body =
            "The mitochondrion steadily produces energy for the rest of the living cell. "
                .repeat(150);
        let blocks = vec![block(BlockKind::Body, &long_body)];

        let drafts = pack_sections(&blocks, &tokenizer, None, None);
        assert!(
            drafts.len() >= 2,
            "long body should be packed into multiple sections"
        );
    }

    #[test]
    fn test_split_sentences_handles_line_breaks_and_carryover() {
        let text =
            "First sentence! Second one\nspans lines. A final\nquestion?\nNo terminator here";
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
    fn test_split_sentences_preserves_decimals() {
        let text = "The value is 3.14 and pi is about 3.14159.";
        let sentences = split_sentences(text);
        assert_eq!(
            sentences,
            vec![String::from("The value is 3.14 and pi is about 3.14159.")]
        );
    }

    #[test]
    fn test_split_sentences_preserves_ellipses() {
        let text = "He paused... then continued. She said… nothing more.";
        let sentences = split_sentences(text);
        assert_eq!(
            sentences,
            vec![
                String::from("He paused... then continued."),
                String::from("She said… nothing more."),
            ]
        );
    }

    #[test]
    fn test_split_sentences_preserves_abbreviations() {
        let text = "See e.g. Smith (2020). Dr. Jones arrived. The U.S. economy grew.";
        let sentences = split_sentences(text);
        assert_eq!(
            sentences,
            vec![
                String::from("See e.g. Smith (2020)."),
                String::from("Dr. Jones arrived."),
                String::from("The U.S. economy grew."),
            ]
        );
    }

    #[test]
    fn test_segment_blocks_packs_long_runs_within_token_bounds() {
        let tokenizer = test_tokenizer();
        let sentence =
            "Photosynthesis converts light energy into chemical energy stored as glucose. ";
        let repeat = 200;
        let blocks = vec![block(BlockKind::Body, &sentence.repeat(repeat))];

        let drafts = segment_blocks(blocks, &tokenizer, None, None);
        assert!(
            drafts.len() >= 2,
            "long body must be split into multiple segments"
        );

        for draft in &drafts {
            assert!(
                token_count(&tokenizer, &draft.text, None) <= MAX_SEGMENT_TOKENS,
                "no segment may exceed MAX_SEGMENT_TOKENS"
            );
        }

        let covered: usize = drafts.iter().map(|draft| draft.text.len()).sum();
        let input_len = sentence.len() * repeat;
        assert!(
            covered >= input_len * 9 / 10,
            "segments must cover essentially all input"
        );
    }

    #[test]
    fn test_merge_undersized_combines_slivers() {
        let tokenizer = test_tokenizer();
        let drafts = vec![
            SegmentDraft {
                heading: None,
                text: String::from("tiny"),
                tokens: token_count(&tokenizer, "tiny", None),
            },
            SegmentDraft {
                heading: None,
                text: String::from("also tiny"),
                tokens: token_count(&tokenizer, "also tiny", None),
            },
            SegmentDraft {
                heading: None,
                text: "large ".repeat(6000),
                tokens: token_count(&tokenizer, &"large ".repeat(6000), None),
            },
        ];
        let merged = merge_undersized(drafts, &tokenizer);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].text, "tiny\nalso tiny");
    }

    #[test]
    fn test_merge_undersized_merges_across_heading_boundaries() {
        // A bare sliver (e.g. a leftover page marker) flush-stuck between
        // differently-headed sections must not survive standalone: merge it
        // into the real section that follows and keep that section's heading.
        let tokenizer = test_tokenizer();
        let drafts = vec![
            SegmentDraft {
                heading: None,
                text: String::from("bare marker"),
                tokens: 6,
            },
            SegmentDraft {
                heading: Some(String::from("COLLEGE")),
                text: String::from("OpenStax provides free, peer-reviewed textbooks."),
                tokens: 400,
            },
        ];
        let merged = merge_undersized(drafts, &tokenizer);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].heading.as_deref(), Some("COLLEGE"));
        assert!(merged[0].text.starts_with("bare marker"));
        assert_eq!(merged[0].tokens, 406);
    }
}
