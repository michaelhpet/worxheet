//! File parsing + segmentation + persistence.
//!
//! Parsers emit typed [`Block`]s (headings vs body) so the segmenter can
//! exploit document structure. Files are parsed on parallel worker threads;
//! resulting segments are stored in one transaction via batched multi-row
//! inserts.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokenizers::Tokenizer;
use tokio::sync::mpsc;

use crate::logging::{self, RunLogs};
use crate::schema::Segment;

use super::segment::{self, Block, BlockKind, SegmentDraft};

/// Wall-clock breakdown of one file's parse, for the ingest log trace.
#[derive(Debug, Clone, Copy, Default)]
pub struct ParseTiming {
    pub open_ms: u128,
    pub extract_ms: u128,
    pub assemble_ms: u128,
}

/// Parse a file into ordered typed blocks. `on_page` (when given) reports
/// `(pages_done, pages_total)` as extraction progresses so ingestion can surface
/// a live progress bar even for a single large file. It is invoked from the
/// extraction worker threads, so it must be `Sync` (thread-safe to call).
pub fn parse_blocks(
    path: &str,
    extension: &str,
    on_page: Option<&(dyn Fn(usize, usize) + Sync)>,
) -> Result<(Vec<Block>, ParseTiming), String> {
    match extension.to_lowercase().as_str() {
        "pdf" => parse_pdf_blocks(path, on_page),
        "pptx" | "docx" | "ppt" | "doc" => {
            parse_office_blocks(path).map(|blocks| (blocks, ParseTiming::default()))
        }
        _ => Err(format!("Unsupported file extension: {}", extension)),
    }
}

/// PDFs: span-level extraction with font-size statistics for heading
/// detection. Falls back to plain page text when layout extraction fails.
/// Pages are extracted in parallel for large documents.
fn parse_pdf_blocks(
    path: &str,
    on_page: Option<&(dyn Fn(usize, usize) + Sync)>,
) -> Result<(Vec<Block>, ParseTiming), String> {
    let open_started = std::time::Instant::now();
    let doc = pdf_oxide::PdfDocument::open(path).map_err(|e| format!("Failed to open PDF: {e}"))?;
    let page_count = doc
        .page_count()
        .map_err(|e| format!("Failed to get page count: {e}"))?;
    let open_ms = open_started.elapsed().as_millis();

    // Pages are now known: emit a start tick so progress + logs move instantly.
    if let Some(on_page) = on_page {
        on_page(0, page_count);
    }

    let doc = Arc::new(doc);
    let parallelism = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(page_count);

    let extract_started = std::time::Instant::now();
    // (page_index, spans, per-page font-size histogram)
    type PageExtract = (usize, Vec<(String, f32)>, std::collections::HashMap<u32, usize>);
    let mut size_histogram: std::collections::HashMap<u32, usize> =
        std::collections::HashMap::new();
    // Slot-indexed spans: each page's data lands directly in its own slot with
    // no shared lock, bounding peak memory to the extracted text itself.
    let mut sorted_pages = vec![Vec::new(); page_count];

    let chunk_size = page_count.div_ceil(parallelism).max(1);
    let chunks: Vec<Vec<usize>> = (0..page_count)
        .collect::<Vec<_>>()
        .chunks(chunk_size)
        .map(|chunk| chunk.to_vec())
        .collect();
    let reported = AtomicUsize::new(0);
    let tick = (page_count / 12).max(1);

    for chunk in &chunks {
        let results: Vec<PageExtract> = std::thread::scope(|s| {
            let mut handles = Vec::with_capacity(chunk.len());
            for &page_index in chunk {
                let doc = &doc;
                let reported = &reported;
                handles.push(s.spawn(move || {
                    // Per-page local histogram + spans: extracted once, merged
                    // once per page, so the shared histogram is never locked.
                    let mut local_hist = std::collections::HashMap::new();
                    let mut page = Vec::new();
                    if let Ok(spans) = doc.extract_spans(page_index) {
                        for span in spans {
                            if span.text.trim().is_empty() {
                                continue;
                            }
                            let size = span.font_size.max(1.0).round() as u32;
                            *local_hist.entry(size).or_default() += span.text.len();
                            page.push((span.text, span.font_size));
                        }
                    }
                    let done = reported.fetch_add(1, Ordering::Relaxed) + 1;
                    if let Some(on_page) = on_page {
                        if done.is_multiple_of(tick) || done == page_count {
                            on_page(done, page_count);
                        }
                    }
                    (page_index, page, local_hist)
                }));
            }
            handles
                .into_iter()
                .map(|handle| handle.join().expect("span extraction task panicked"))
                .collect()
        });
        for (page_index, spans, local_hist) in results {
            sorted_pages[page_index] = spans;
            for (size, count) in local_hist {
                *size_histogram.entry(size).or_default() += count;
            }
        }
    }
    let extract_ms = extract_started.elapsed().as_millis();

    let assemble_started = std::time::Instant::now();
    let body_size = size_histogram
        .iter()
        .max_by_key(|(_, count)| **count)
        .map(|(size, _)| *size as f32)
        .unwrap_or(12.0);

    let mut blocks = Vec::new();
    let mut pending_body = String::new();

    for page in &sorted_pages {
        if page.is_empty() {
            continue;
        }

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
    let assemble_ms = assemble_started.elapsed().as_millis();

    // Layout extraction produced nothing usable — fall back to auto text.
    if blocks
        .iter()
        .filter(|block| matches!(block.kind, BlockKind::Heading(_) | BlockKind::Body))
        .all(|block| block.text.trim().is_empty())
    {
        return parse_pdf_plain(path, on_page);
    }

    Ok((
        blocks,
        ParseTiming {
            open_ms,
            extract_ms,
            assemble_ms,
        },
    ))
}

fn parse_pdf_plain(
    path: &str,
    on_page: Option<&(dyn Fn(usize, usize) + Sync)>,
) -> Result<(Vec<Block>, ParseTiming), String> {
    let open_started = std::time::Instant::now();
    let doc = pdf_oxide::PdfDocument::open(path).map_err(|e| format!("Failed to open PDF: {e}"))?;
    let page_count = doc
        .page_count()
        .map_err(|e| format!("Failed to get page count: {e}"))?;
    let open_ms = open_started.elapsed().as_millis();

    if let Some(on_page) = on_page {
        on_page(0, page_count);
    }

    let doc = Arc::new(doc);
    let parallelism = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(page_count);

    let extract_started = std::time::Instant::now();
    let page_texts: Mutex<Vec<(usize, String)>> = Mutex::new(Vec::new());

    let chunk_size = page_count.div_ceil(parallelism).max(1);
    let chunks: Vec<Vec<usize>> = (0..page_count)
        .collect::<Vec<_>>()
        .chunks(chunk_size)
        .map(|chunk| chunk.to_vec())
        .collect();
    let reported = AtomicUsize::new(0);
    let tick = (page_count / 12).max(1);

    for chunk in &chunks {
        std::thread::scope(|s| {
            let doc = &doc;
            let page_texts = &page_texts;
            let reported = &reported;
            for &page_index in chunk {
                s.spawn(move || {
                    let text = doc
                        .extract_text_auto(page_index)
                        .map(|t| t.trim().to_string())
                        .unwrap_or_default();
                    if !text.is_empty() {
                        page_texts.lock().unwrap().push((page_index, text));
                    }
                    let done = reported.fetch_add(1, Ordering::Relaxed) + 1;
                    if let Some(on_page) = on_page {
                        if done.is_multiple_of(tick) || done == page_count {
                            on_page(done, page_count);
                        }
                    }
                });
            }
        });
    }
    let extract_ms = extract_started.elapsed().as_millis();

    let assemble_started = std::time::Instant::now();
    let mut page_texts = page_texts.into_inner().unwrap();
    page_texts.sort_by_key(|(idx, _)| *idx);
    let blocks = page_texts
        .into_iter()
        .map(|(_page_index, text)| Block {
            kind: BlockKind::Body,
            text,
        })
        .collect();
    let assemble_ms = assemble_started.elapsed().as_millis();

    Ok((
        blocks,
        ParseTiming {
            open_ms,
            extract_ms,
            assemble_ms,
        },
    ))
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

/// Copy chunks from an existing file that shares the same content hash as one
/// of `file_ids`, so a duplicate source is not re-parsed/segmented per
/// worksheet. Chunks are copied in their original order and assigned fresh
/// ids/positions for this worksheet. Returns the file ids that still need a
/// real parse (no hash, or no matching chunked source) and the next position
/// to continue from.
pub async fn reuse_chunks(
    pool: &sqlx::SqlitePool,
    worksheet_id: &str,
    file_ids: &[String],
    start_position: i32,
) -> Result<(Vec<String>, i32), String> {
    if file_ids.is_empty() {
        return Ok((Vec::new(), start_position));
    }

    // Bulk-load all file identity keys in one query.
    let ids_csv: String = file_ids
        .iter()
        .map(|id| format!("'{id}'"))
        .collect::<Vec<_>>()
        .join(",");
    let identity_rows: Vec<(String, Option<String>)> = sqlx::query_as(&format!(
        "SELECT id, sha256 FROM files WHERE worksheet_id = ? AND id IN ({ids_csv})"
    ))
    .bind(worksheet_id)
    .fetch_all(pool)
    .await
    .map_err(|_| String::from("Failed to query file identity keys"))?;

    let identity_map: std::collections::HashMap<String, Option<String>> =
        identity_rows.into_iter().collect();

    let mut remaining = Vec::new();
    let mut position = start_position;
    let mut all_insert_rows: Vec<(String, String, i32, Option<String>, String)> = Vec::new();

    for file_id in file_ids {
        let identity = identity_map.get(file_id).and_then(|h| h.clone());
        let Some(identity) = identity else {
            remaining.push(file_id.clone());
            continue;
        };

        // Find an already-chunked file with the same identity key in another
        // worksheet.
        let source: Option<(String,)> = sqlx::query_as(
            "SELECT c.file_id
             FROM chunks c
             JOIN files f ON f.id = c.file_id
             WHERE f.sha256 = ? AND c.worksheet_id <> ? AND c.worksheet_id IS NOT NULL
             GROUP BY c.file_id
             ORDER BY MIN(c.position)
             LIMIT 1",
        )
        .bind(&identity)
        .bind(worksheet_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| String::from("Failed to look up reusable chunks"))?;

        let Some((source_file_id,)) = source else {
            remaining.push(file_id.clone());
            continue;
        };

        let rows: Vec<(Option<String>, String)> = sqlx::query_as(
            "SELECT heading, text FROM chunks
             WHERE file_id = ?
             ORDER BY position",
        )
        .bind(&source_file_id)
        .fetch_all(pool)
        .await
        .map_err(|_| String::from("Failed to load reusable chunks"))?;

        for (heading, text) in rows {
            all_insert_rows.push((
                ulid::Ulid::new().to_string(),
                file_id.clone(),
                position,
                heading,
                text,
            ));
            position += 1;
        }
    }

    if !all_insert_rows.is_empty() {
        let mut transaction = pool
            .begin()
            .await
            .map_err(|_| String::from("Failed to begin reuse transaction"))?;

        let batch_size = 400;
        for chunk in all_insert_rows.chunks(batch_size) {
            let mut builder = sqlx::QueryBuilder::new(
                "INSERT INTO chunks (id, worksheet_id, file_id, position, heading, text) ",
            );
            builder.push_values(chunk, |mut b, (id, fid, pos, heading, text)| {
                b.push_bind(id)
                    .push_bind(worksheet_id)
                    .push_bind(fid)
                    .push_bind(pos)
                    .push_bind(heading)
                    .push_bind(text);
            });
            builder
                .build()
                .execute(&mut *transaction)
                .await
                .map_err(|_| String::from("Failed to batch insert reused chunks"))?;
        }

        transaction
            .commit()
            .await
            .map_err(|_| String::from("Failed to commit reused chunks"))?;
    }

    sqlx::query("UPDATE worksheets SET updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(worksheet_id)
        .execute(pool)
        .await
        .map_err(|_| String::from("Failed to update worksheet"))?;

    Ok((remaining, position))
}

pub async fn process_files(
    pool: &sqlx::SqlitePool,
    worksheet_id: &str,
    file_ids: &[String],
    start_position: i32,
    tokenizer: Arc<Tokenizer>,
    mut on_progress: Option<Box<dyn FnMut(usize, usize) + Send>>,
    logs: Option<Arc<RunLogs>>,
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
        for (offset, (file_id, path, extension)) in metadata[start..end].iter().enumerate() {
            let index = start + offset;
            let tx = tx.clone();
            let tokenizer = tokenizer.clone();
            let logs = logs.clone();
            let path = path.clone();
            let extension = extension.clone();
            let file_id = file_id.clone();
            let worksheet_id = worksheet_id.to_string();
            tauri::async_runtime::spawn_blocking(move || {
                let result = (|| -> Result<Vec<SegmentDraft>, String> {
                    // File reads: parse and log the extracted shape.
                    if let Some(logs) = &logs {
                        let started_record = serde_json::json!({
                            "timestamp": logging::rfc3339_utc(),
                            "worksheet_id": worksheet_id,
                            "file_id": file_id,
                            "path": path,
                            "extension": extension,
                            "phase": "started",
                        });
                        logs.write_json(
                            "file_reads",
                            &format!(
                                "parse_started_{}",
                                logging::sanitize_label(&file_id)
                            ),
                            &started_record,
                        );
                    }
                    let parse_started = std::time::Instant::now();
                    let pages = AtomicUsize::new(0usize);
                    let page_sender = tx.clone();
                    // `on_page` fires from extraction worker threads, so it must
                    // be Sync: an atomic page count + a channel sender suffice.
                    let on_parse = |done: usize, total: usize| {
                        pages.store(total, Ordering::SeqCst);
                        let _ = page_sender.blocking_send(Msg::Progress(done, total));
                    };
                    let (blocks, parse_timing) =
                        parse_blocks(&path, &extension, Some(&on_parse))?;
                    if let Some(logs) = &logs {
                        let mut by_kind = std::collections::BTreeMap::new();
                        for block in &blocks {
                            let kind = match block.kind {
                                BlockKind::Heading(_) => "heading",
                                BlockKind::Body => "body",
                            };
                            *by_kind.entry(kind).or_insert(0usize) += 1;
                        }
                        let record = serde_json::json!({
                            "timestamp": logging::rfc3339_utc(),
                            "worksheet_id": worksheet_id,
                            "file_id": file_id,
                            "path": path,
                            "extension": extension,
                            "blocks": by_kind,
                            "pages": pages.load(Ordering::SeqCst),
                            "duration_ms": parse_started.elapsed().as_millis(),
                            "phase_breakdown": {
                                "open_ms": parse_timing.open_ms,
                                "extract_ms": parse_timing.extract_ms,
                                "assemble_ms": parse_timing.assemble_ms,
                            },
                        });
                        logs.write_json("file_reads", &logging::sanitize_label(&file_id), &record);
                    }

                    // Segmentation: tokenize once per file and log the cost.
                    let seg_started = std::time::Instant::now();
                    let mut trace = segment::TokenizeTrace::default();
                    let tx_seg = tx.clone();
                    let drafts = segment::segment_blocks(
                        blocks,
                        &tokenizer,
                        Some(&mut move |bd, bt| {
                            let _ = tx_seg.blocking_send(Msg::Progress(bd, bt));
                        }),
                        Some(&mut trace),
                    );
                    if let Some(logs) = &logs {
                        let token_record = serde_json::json!({
                            "timestamp": logging::rfc3339_utc(),
                            "duration_ms": seg_started.elapsed().as_millis(),
                            "calls": trace.calls,
                            "tokens": trace.tokens,
                        });
                        logs.write_json(
                            "tokenization",
                            &logging::sanitize_label(&file_id),
                            &token_record,
                        );

                        let segments: Vec<_> = drafts
                            .iter()
                            .enumerate()
                            .map(|(position, draft)| {
                                serde_json::json!({
                                    "timestamp": logging::rfc3339_utc(),
                                    "position": position,
                                    "heading": draft.heading,
                                    "tokens": draft.tokens,
                                    "chars": draft.text.chars().count(),
                                })
                            })
                            .collect();
                        let segment_record = serde_json::json!({
                            "timestamp": logging::rfc3339_utc(),
                            "count": segments.len(),
                            "total_tokens": drafts.iter().map(|d| d.tokens).sum::<usize>(),
                            "segments": segments,
                        });
                        logs.write_json(
                            "segmentation",
                            &logging::sanitize_label(&file_id),
                            &segment_record,
                        );
                    }
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
    let mut insert_rows: Vec<(String, String, String, i32, Option<String>, String)> =
        Vec::with_capacity(flat.len());
    let mut position_counter = start_position;

    for (file_index, (file_id, _, _)) in metadata.iter().enumerate() {
        let Some(drafts) = segmented[file_index].as_ref() else {
            continue;
        };
        for draft in drafts {
            let heading_text: Option<String> = draft
                .heading
                .clone()
                .filter(|heading| !heading.trim().is_empty());

            let segment_id = ulid::Ulid::new().to_string();

            insert_rows.push((
                segment_id.clone(),
                worksheet_id.to_string(),
                file_id.clone(),
                position_counter,
                heading_text.clone(),
                draft.text.clone(),
            ));

            all_segments.push(Segment {
                id: segment_id,
                worksheet_id: worksheet_id.to_string(),
                file_id: file_id.clone(),
                position: position_counter,
                heading: heading_text,
                text: draft.text.clone(),
            });
            position_counter += 1;
        }
    }

    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| String::from("Failed to begin ingest transaction"))?;

    let batch_size = 400;
    for chunk in insert_rows.chunks(batch_size) {
        let mut builder = sqlx::QueryBuilder::new(
            "INSERT INTO chunks (id, worksheet_id, file_id, position, heading, text) ",
        );
        builder.push_values(chunk, |mut b, (id, ws, fid, pos, heading, text)| {
            b.push_bind(id)
                .push_bind(ws)
                .push_bind(fid)
                .push_bind(pos)
                .push_bind(heading)
                .push_bind(text);
        });
        builder
            .build()
            .execute(&mut *transaction)
            .await
            .map_err(|_| String::from("Failed to batch insert segments"))?;
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
        assert!(parse_blocks("test.txt", "txt", None).is_err());
    }

    #[test]
    fn test_parse_missing_file() {
        let path = format!("{}/nonexistent.pdf", FIXTURE_DIR);
        assert!(parse_blocks(&path, "pdf", None).is_err());
    }

    async fn setup_db() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("in-memory pool");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("migrations");
        pool
    }

    async fn seed_file(
        pool: &sqlx::SqlitePool,
        worksheet_id: &str,
        sha256: Option<&str>,
    ) -> String {
        let id = ulid::Ulid::new().to_string();
        sqlx::query(
            "INSERT INTO files (id, worksheet_id, path, name, extension, size, sha256)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(worksheet_id)
        .bind("/tmp/sample.pdf")
        .bind("sample.pdf")
        .bind("pdf")
        .bind(10i64)
        .bind(sha256)
        .execute(pool)
        .await
        .unwrap();
        id
    }

    async fn seed_chunk(
        pool: &sqlx::SqlitePool,
        worksheet_id: &str,
        file_id: &str,
        position: i32,
        text: &str,
    ) {
        sqlx::query(
            "INSERT INTO chunks (id, worksheet_id, file_id, position, heading, text)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(ulid::Ulid::new().to_string())
        .bind(worksheet_id)
        .bind(file_id)
        .bind(position)
        .bind(Option::<String>::None)
        .bind(text)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_reuse_chunks_copies_matching_source() {
        let pool = setup_db().await;

        let ws_a = ulid::Ulid::new().to_string();
        sqlx::query("INSERT INTO worksheets (id, name) VALUES (?, ?)")
            .bind(&ws_a)
            .bind("a")
            .execute(&pool)
            .await
            .unwrap();
        let source_file = seed_file(&pool, &ws_a, Some("deadbeef")).await;
        seed_chunk(&pool, &ws_a, &source_file, 0, "first chunk").await;
        seed_chunk(&pool, &ws_a, &source_file, 1, "second chunk").await;

        let ws_b = ulid::Ulid::new().to_string();
        sqlx::query("INSERT INTO worksheets (id, name) VALUES (?, ?)")
            .bind(&ws_b)
            .bind("b")
            .execute(&pool)
            .await
            .unwrap();
        let pending = seed_file(&pool, &ws_b, Some("deadbeef")).await;

        let (remaining, next_pos) = reuse_chunks(&pool, &ws_b, std::slice::from_ref(&pending), 0)
            .await
            .unwrap();

        assert!(remaining.is_empty(), "all files should be deduped");
        assert_eq!(next_pos, 2);

        let copied: Vec<(String, i32)> = sqlx::query_as(
            "SELECT text, position FROM chunks WHERE worksheet_id = ? ORDER BY position",
        )
        .bind(&ws_b)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(copied.len(), 2);
        assert_eq!(copied[0], ("first chunk".to_string(), 0));
        assert_eq!(copied[1], ("second chunk".to_string(), 1));
    }

    #[tokio::test]
    async fn test_reuse_chunks_leaves_nonmatching_for_parse() {
        let pool = setup_db().await;

        let ws_a = ulid::Ulid::new().to_string();
        sqlx::query("INSERT INTO worksheets (id, name) VALUES (?, ?)")
            .bind(&ws_a)
            .bind("a")
            .execute(&pool)
            .await
            .unwrap();
        let source_file = seed_file(&pool, &ws_a, Some("aaaa")).await;
        seed_chunk(&pool, &ws_a, &source_file, 0, "other").await;

        let ws_b = ulid::Ulid::new().to_string();
        sqlx::query("INSERT INTO worksheets (id, name) VALUES (?, ?)")
            .bind(&ws_b)
            .bind("b")
            .execute(&pool)
            .await
            .unwrap();
        let no_match = seed_file(&pool, &ws_b, Some("bbbb")).await;
        let no_hash = seed_file(&pool, &ws_b, None).await;

        let (remaining, next_pos) =
            reuse_chunks(&pool, &ws_b, &[no_match.clone(), no_hash.clone()], 5)
                .await
                .unwrap();

        assert_eq!(remaining, vec![no_match, no_hash]);
        assert_eq!(next_pos, 5, "no chunks copied, position unchanged");
    }
}
