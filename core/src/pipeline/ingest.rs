//! File parsing + segmentation + persistence.
//!
//! Parsers emit typed [`Block`]s (headings vs body) so the segmenter can
//! exploit document structure. Files are parsed on parallel worker threads;
//! resulting segments are stored in one transaction.

use tokenizers::Tokenizer;
use tokio::sync::mpsc;

use crate::schema::Segment;

use super::segment::{self, Block, BlockKind, SegmentDraft};

/// Parse a file into ordered typed blocks. `on_page` (when given) reports
/// `(pages_done, pages_total)` as extraction progresses so ingestion can surface
/// a live progress bar even for a single large file.
pub fn parse_blocks(
    path: &str,
    extension: &str,
    on_page: Option<&mut dyn FnMut(usize, usize)>,
) -> Result<Vec<Block>, String> {
    match extension.to_lowercase().as_str() {
        "pdf" => parse_pdf_blocks(path, on_page),
        "pptx" | "docx" | "ppt" | "doc" => parse_office_blocks(path),
        _ => Err(format!("Unsupported file extension: {}", extension)),
    }
}

/// PDFs: span-level extraction with font-size statistics for heading
/// detection. Falls back to plain page text when layout extraction fails.
fn parse_pdf_blocks(
    path: &str,
    mut on_page: Option<&mut dyn FnMut(usize, usize)>,
) -> Result<Vec<Block>, String> {
    let doc = pdf_oxide::PdfDocument::open(path).map_err(|e| format!("Failed to open PDF: {e}"))?;
    let page_count = doc
        .page_count()
        .map_err(|e| format!("Failed to get page count: {e}"))?;

    // First pass: collect spans to learn the dominant (body) font size.
    let mut size_histogram: std::collections::HashMap<u32, usize> =
        std::collections::HashMap::new();
    let mut page_spans: Vec<Vec<(String, f32)>> = Vec::with_capacity(page_count);
    for page_index in 0..page_count {
        if let Some(on_page) = on_page.as_deref_mut() {
            on_page(page_index + 1, page_count);
        }
        let mut page = Vec::new();
        if let Ok(spans) = doc.extract_spans(page_index) {
            for span in spans {
                if span.text.trim().is_empty() {
                    continue;
                }
                *size_histogram
                    .entry(span.font_size.max(1.0).round() as u32)
                    .or_default() += span.text.len();
                page.push((span.text, span.font_size));
            }
        }
        page_spans.push(page);
    }

    let body_size = size_histogram
        .iter()
        .max_by_key(|(_, count)| **count)
        .map(|(size, _)| *size as f32)
        .unwrap_or(12.0);

    let mut blocks = Vec::new();
    let mut pending_body = String::new();

    for (page_index, page) in page_spans.iter().enumerate() {
        if page.is_empty() {
            continue;
        }
        blocks.push(Block {
            kind: BlockKind::Body,
            text: format!("[Page {}]", page_index + 1),
        });

        for (text, size) in page {
            let is_heading = *size >= body_size * 1.25 && text.len() < 120;
            if is_heading {
                if !pending_body.trim().is_empty() {
                    blocks.push(Block {
                        kind: BlockKind::Body,
                        text: std::mem::take(&mut pending_body),
                    });
                }
                blocks.push(Block {
                    kind: BlockKind::Heading(1),
                    text: text.clone(),
                });
            } else {
                pending_body.push_str(text);
                pending_body.push(' ');
            }
        }
        if !pending_body.trim().is_empty() {
            blocks.push(Block {
                kind: BlockKind::Body,
                text: std::mem::take(&mut pending_body),
            });
        }
    }

    // Layout extraction produced nothing usable — fall back to auto text.
    if blocks
        .iter()
        .filter(|block| matches!(block.kind, BlockKind::Heading(_) | BlockKind::Body))
        .all(|block| block.text.trim().is_empty())
    {
        return parse_pdf_plain(path, on_page);
    }

    Ok(blocks)
}

fn parse_pdf_plain(
    path: &str,
    mut on_page: Option<&mut dyn FnMut(usize, usize)>,
) -> Result<Vec<Block>, String> {
    let doc = pdf_oxide::PdfDocument::open(path).map_err(|e| format!("Failed to open PDF: {e}"))?;
    let page_count = doc
        .page_count()
        .map_err(|e| format!("Failed to get page count: {e}"))?;
    let mut blocks = Vec::new();
    for page_index in 0..page_count {
        if let Some(on_page) = on_page.as_deref_mut() {
            on_page(page_index + 1, page_count);
        }
        let text = doc
            .extract_text_auto(page_index)
            .map_err(|e| format!("Failed to extract page {page_index}: {e}"))?;
        if text.trim().is_empty() {
            continue;
        }
        blocks.push(Block {
            kind: BlockKind::Body,
            text: format!("[Page {}]\n{}", page_index + 1, text.trim()),
        });
    }
    Ok(blocks)
}

/// Office formats: markdown export preserves headings (`#`) and slide titles;
/// split on those markers into structured blocks.
fn parse_office_blocks(path: &str) -> Result<Vec<Block>, String> {
    let markdown =
        office_oxide::to_markdown(path).map_err(|e| format!("Failed to extract text: {e}"))?;

    let mut blocks = Vec::new();
    let mut pending_body = String::new();

    for line in markdown.lines() {
        let trimmed = line.trim_start();
        let heading_level = trimmed.chars().take_while(|c| *c == '#').count();
        let looks_like_heading = heading_level > 0
            && heading_level <= 6
            && trimmed
                .get(heading_level..)
                .is_some_and(|rest| rest.starts_with(' '));

        if looks_like_heading {
            if !pending_body.trim().is_empty() {
                blocks.push(Block {
                    kind: BlockKind::Body,
                    text: std::mem::take(&mut pending_body),
                });
            }
            blocks.push(Block {
                kind: BlockKind::Heading(heading_level.min(u8::MAX as usize) as u8),
                text: trimmed[heading_level..]
                    .trim_start()
                    .trim_end_matches('#')
                    .trim()
                    .to_string(),
            });
        } else {
            pending_body.push_str(line);
            pending_body.push('\n');
        }
    }
    if !pending_body.trim().is_empty() {
        blocks.push(Block {
            kind: BlockKind::Body,
            text: pending_body,
        });
    }

    if blocks.is_empty() {
        // Structure-free fallback: whole document as one body block; drift
        // segmentation will carve it up.
        let plain =
            office_oxide::extract_text(path).map_err(|e| format!("Failed to extract text: {e}"))?;
        if !plain.trim().is_empty() {
            blocks.push(Block {
                kind: BlockKind::Body,
                text: plain,
            });
        }
    }

    Ok(blocks)
}

/// Parse every file, segment it, and persist the segments in source order.
/// Reports progress as each file completes.
pub async fn unchunked_files(
    pool: &sqlx::SqlitePool,
    worksheet_id: &str,
    file_ids: &[String],
) -> Result<Vec<String>, String> {
    if file_ids.is_empty() {
        return Ok(Vec::new());
    }

    let chunked: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT file_id FROM chunks WHERE worksheet_id = ?")
            .bind(worksheet_id)
            .fetch_all(pool)
            .await
            .map_err(|_| String::from("Failed to query existing chunks"))?;

    let chunked_set: std::collections::HashSet<&str> = chunked.iter().map(String::as_str).collect();
    Ok(file_ids
        .iter()
        .filter(|id| !chunked_set.contains(id.as_str()))
        .cloned()
        .collect())
}

pub async fn next_segment_position(
    pool: &sqlx::SqlitePool,
    worksheet_id: &str,
) -> Result<i32, String> {
    let (max_position,): (Option<i32>,) =
        sqlx::query_as("SELECT MAX(position) FROM chunks WHERE worksheet_id = ?")
            .bind(worksheet_id)
            .fetch_one(pool)
            .await
            .map_err(|_| String::from("Failed to query segment position"))?;

    Ok(max_position.unwrap_or(-1) + 1)
}

pub async fn process_files(
    pool: &sqlx::SqlitePool,
    worksheet_id: &str,
    file_ids: &[String],
    start_position: i32,
    tokenizer: Tokenizer,
    mut on_progress: Option<Box<dyn FnMut(usize, usize) + Send>>,
) -> Result<Vec<Segment>, String> {
    let total_files = file_ids.len();
    if total_files == 0 {
        return Ok(Vec::new());
    }

    // Load metadata up front so parsing never touches the database.
    let mut metadata = Vec::with_capacity(total_files);
    for file_id in file_ids {
        let row = sqlx::query_as::<_, (String, String)>(
            "SELECT path, extension FROM files WHERE id = ? AND worksheet_id = ?",
        )
        .bind(file_id)
        .bind(worksheet_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| format!("Failed to query file {file_id}"))?
        .ok_or_else(|| format!("File not found: {file_id}"))?;
        metadata.push((file_id.clone(), row.0, row.1));
    }

    let max_parallel = total_files.min(
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4),
    );

    let mut segmented: Vec<Option<Vec<SegmentDraft>>> = vec![None; total_files];

    // Single channel carrying both live page progress and file completions so
    // the async side can report progress while workers parse.
    enum Msg {
        Progress(usize, usize),
        Done(usize, Result<Vec<SegmentDraft>, String>),
    }
    let capacity = total_files.max(1) * 4096;
    let (tx, mut rx) = mpsc::channel::<Msg>(capacity);

    let mut done = 0usize;
    let mut start = 0;
    while start < total_files {
        let end = (start + max_parallel).min(total_files);
        for (offset, (_, path, extension)) in metadata[start..end].iter().enumerate() {
            let index = start + offset;
            let tx = tx.clone();
            let tokenizer = tokenizer.clone();
            let path = path.clone();
            let extension = extension.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let result = (|| -> Result<Vec<SegmentDraft>, String> {
                    let tx_page = tx.clone();
                    let blocks = parse_blocks(&path, &extension, Some(&mut move |pd, pt| {
                        let _ = tx_page.blocking_send(Msg::Progress(pd, pt));
                    }))?;
                    let tx_seg = tx.clone();
                    let drafts = segment::segment_blocks(
                        blocks,
                        &tokenizer,
                        Some(&mut move |bd, bt| {
                            let _ = tx_seg.blocking_send(Msg::Progress(bd, bt));
                        }),
                    );
                    Ok(drafts)
                })();
                let _ = tx.blocking_send(Msg::Done(index, result));
            });
        }

        while done < end {
            match rx
                .recv()
                .await
                .ok_or_else(|| String::from("Parse channel closed unexpectedly"))?
            {
                Msg::Progress(pages_done, pages_total) => {
                    if let Some(on_progress) = on_progress.as_deref_mut() {
                        on_progress(pages_done, pages_total);
                    }
                }
                Msg::Done(index, result) => {
                    segmented[index] = Some(result?);
                    done += 1;
                }
            }
        }
        start = end;
    }

    // Flatten drafts in document order, then persist in one transaction.
    let flat: Vec<&SegmentDraft> = segmented.iter().flatten().flatten().collect();
    if flat.is_empty() {
        return Ok(Vec::new());
    }

    let mut all_segments = Vec::with_capacity(flat.len());
    let mut position_counter = start_position;

    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| String::from("Failed to begin ingest transaction"))?;

    for (file_index, (file_id, _, _)) in metadata.iter().enumerate() {
        let Some(drafts) = segmented[file_index].take() else {
            continue;
        };
        for draft in drafts {
            let heading_text: Option<String> = draft
                .heading
                .clone()
                .filter(|heading| !heading.trim().is_empty());

            let segment_id = ulid::Ulid::new().to_string();
            sqlx::query(
                "INSERT INTO chunks (id, worksheet_id, file_id, position, heading, text)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&segment_id)
            .bind(worksheet_id)
            .bind(file_id)
            .bind(position_counter)
            .bind(&heading_text)
            .bind(&draft.text)
            .execute(&mut *transaction)
            .await
            .map_err(|_| String::from("Failed to insert segment"))?;

            all_segments.push(Segment {
                id: segment_id,
                worksheet_id: worksheet_id.to_string(),
                file_id: file_id.clone(),
                position: position_counter,
                heading: heading_text,
                text: draft.text,
            });
            position_counter += 1;
        }
    }

    transaction
        .commit()
        .await
        .map_err(|_| String::from("Failed to commit ingested segments"))?;

    sqlx::query("UPDATE worksheets SET updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(worksheet_id)
        .execute(pool)
        .await
        .map_err(|_| String::from("Failed to update worksheet"))?;

    Ok(all_segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

    #[test]
    fn test_parse_unsupported_extension() {
        assert!(parse_blocks("test.txt", "txt").is_err());
    }

    #[test]
    fn test_parse_missing_file() {
        let path = format!("{}/nonexistent.pdf", FIXTURE_DIR);
        assert!(parse_blocks(&path, "pdf").is_err());
    }
}
