//! File parsing + segmentation + persistence.
//!
//! Parsers emit typed [`Block`]s (headings vs body) so the segmenter can
//! exploit document structure. Files are parsed on parallel worker threads;
//! resulting segments are stored in one transaction via batched multi-row
//! inserts.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use liteparse::layout::{LayoutBlock, LayoutCell};
use liteparse::ocr_merge::{ComplexityReason, PageComplexityStats};
use liteparse::types::PdfInput;
use liteparse::{
    LiteParse, LiteParseConfig, OutputFormat, ParsedPage, DEFAULT_PAGE_BATCH_SIZE,
};
use tokenizers::Tokenizer;
use tokio::sync::mpsc;

use crate::logging::{self, RunLogs};
use crate::schema::Segment;

use super::segment::{self, Block, BlockKind, SegmentDraft};

/// Future that resolves once the pipeline stop flag is set. With no flag it
/// pends forever so `tokio::select!` callers simply wait on the other branch.
async fn cancel_requested(cancel: &Option<Arc<std::sync::atomic::AtomicBool>>) {
    match cancel {
        Some(flag) => loop {
            if flag.load(Ordering::Relaxed) {
                return;
            }
            tokio::task::yield_now().await;
        },
        None => std::future::pending().await,
    }
}

/// PDF and image inputs routed to liteparse (PDFs are extracted directly;
/// photos/scans are converted to PDF in-process before OCR).
const DOCUMENT_EXTENSIONS: &[&str] = &[
    "pdf", "jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp", "svg",
];
/// iPhone/HEIF photos cannot be read by liteparse, so they are converted to
/// JPEG first with macOS's bundled `sips` tool.
const HEIC_EXTENSIONS: &[&str] = &["heic", "heif"];

/// OCR (raster render + Tesseract recognition) is by far the dominant parse
/// cost. For PDFs it is enabled only when at least this share of pages
/// genuinely lack a usable native text layer (scanned/photographed pages);
/// born-digital books parse without OCR even when figure-heavy.
const OCR_NEEDED_PAGE_FRACTION: f32 = 0.5;

/// Parse a file into ordered typed blocks. `on_page` (when given) reports
/// `(pages_done, pages_total)` as parsing progresses so ingestion can surface a
/// live progress bar even for a single large file. It is invoked from the parse
/// worker thread, so it must be `Sync` (thread-safe to call).
pub fn parse_blocks(
    path: &str,
    extension: &str,
    on_page: Option<&(dyn Fn(usize, usize) + Sync)>,
) -> Result<Vec<Block>, String> {
    let ext = extension.to_lowercase();
    if DOCUMENT_EXTENSIONS.contains(&ext.as_str()) {
        return liteparse_blocks(path, ext == "pdf", on_page);
    }
    if HEIC_EXTENSIONS.contains(&ext.as_str()) {
        let jpeg = heic_to_jpeg(path)?;
        return liteparse_blocks(&jpeg, false, on_page);
    }
    match ext.as_str() {
        "pptx" | "docx" | "ppt" | "doc" => parse_office_blocks(path),
        _ => Err(format!("Unsupported file extension: {}", extension)),
    }
}

/// Parse PDFs and images via liteparse. Text PDFs use its layout classifier;
/// scanned/handwritten pages go through OCR (bundled Tesseract by default, or
/// a configured `ocr_server_url` when one is set). OCR is gated per PDF (see
/// [`ocr_enabled_for_pdf`]) so a born-digital book with figures — whose pages
/// get flagged for OCR just for containing images — does not pay for a full
/// raster+recognition pass over its entire text layer. Images always take OCR
/// (single pages, no native text). Pages are processed in bounded batches so a
/// large document never holds every rendered page in memory; `on_page` advances
/// once per finished batch.
fn liteparse_blocks(
    path: &str,
    pdf: bool,
    on_page: Option<&(dyn Fn(usize, usize) + Sync)>,
) -> Result<Vec<Block>, String> {
    // OCR runs on bundled Tesseract unless `WORXHEET_OCR_SERVER_URL` points at
    // an EasyOCR/PaddleOCR HTTP sidecar (the higher-quality path for cursive
    // handwriting). Exposing the rest of the liteparse knobs is TBD work.
    let ocr_server_url = std::env::var("WORXHEET_OCR_SERVER_URL")
        .ok()
        .filter(|url| !url.trim().is_empty());
    let config = LiteParseConfig {
        ocr_enabled: true,
        ocr_server_url,
        ocr_failure_fatal: false,
        continue_on_page_error: true,
        quiet: true,
        max_pages: 100_000,
        dpi: 300.0,
        extract_blocks: true,
        output_format: OutputFormat::Markdown,
        ..Default::default()
    };
    let ocr_enabled = if pdf {
        let probe = LiteParse::new(config.clone());
        ocr_enabled_for_pdf(&probe, path)
    } else {
        true
    };
    let parser = LiteParse::new(LiteParseConfig {
        ocr_enabled,
        ..config
    });

    let mut session = tauri::async_runtime::block_on(parser.open_batch_session(
        PdfInput::Path(path.to_string()),
        DEFAULT_PAGE_BATCH_SIZE,
    ))
    .map_err(|e| format!("Failed to open document {path}: {e}"))?;

    // Pages are now known: emit a start tick so progress + logs move instantly.
    let total_pages = session.total_pages() as usize;
    if let Some(on_page) = on_page {
        on_page(0, total_pages);
    }

    let mut blocks = Vec::new();
    while let Some(batch) = tauri::async_runtime::block_on(session.next_batch())
        .map_err(|e| format!("Failed to parse document {path}: {e}"))?
    {
        for page in &batch.result.pages {
            blocks.extend(blocks_from_page(page));
        }
        if let Some(on_page) = on_page {
            on_page(batch.end_page as usize, total_pages);
        }
    }
    Ok(blocks)
}

/// A page genuinely needs OCR when its native text layer is missing or
/// unusable. Pages flagged only because they *contain* an inline raster
/// (`EmbeddedImages`) — and pages whose text is thin but readable
/// (`SparseText`) — still yield usable text from the native layer, so they
/// must not count: figure-heavy born-digital books would otherwise re-enable
/// OCR over a complete text layer, the exact pathology behind the multi-minute
/// parses.
fn page_needs_ocr(stats: &PageComplexityStats) -> bool {
    stats.reasons.iter().any(|reason| {
        matches!(
            reason,
            ComplexityReason::Scanned
                | ComplexityReason::NoText
                | ComplexityReason::Garbled
                | ComplexityReason::VectorText
        )
    })
}

/// Fraction of pages whose native text layer genuinely needs OCR recovery.
fn ocr_needed_page_fraction(stats: &[PageComplexityStats]) -> f32 {
    if stats.is_empty() {
        return 0.0;
    }
    stats.iter().filter(|s| page_needs_ocr(s)).count() as f32 / stats.len() as f32
}

/// Decide whether a PDF should run OCR: only when at least
/// [`OCR_NEEDED_PAGE_FRACTION`] of its pages genuinely lack usable native text
/// (scanned/photographed documents). Uses liteparse's cheap pre-OCR pass —
/// a native-text + page-object walk, no rendering — so it costs seconds even
/// for a 1600-page book. Falls back to OCR-enabled on any failure so nothing
/// silently loses content.
fn ocr_enabled_for_pdf(parser: &LiteParse, path: &str) -> bool {
    let stats = tauri::async_runtime::block_on(parser.is_complex(PdfInput::Path(
        path.to_string(),
    )));
    match stats {
        Ok(stats) => ocr_needed_page_fraction(&stats) >= OCR_NEEDED_PAGE_FRACTION,
        Err(_) => true,
    }
}

/// Convert one parsed page into typed blocks. The layout classifier is
/// preferred: its `heading`/`paragraph`/… blocks carry reading order and
/// heading levels. OCR-only (scanned/handwritten) pages have no layout
/// decomposition, so we fall back to the markdown emitter, then to raw text.
fn blocks_from_page(page: &ParsedPage) -> Vec<Block> {
    if let Some(layout) = &page.blocks {
        let blocks: Vec<Block> = layout.iter().filter_map(layout_block_to_block).collect();
        if !blocks.is_empty() {
            return blocks;
        }
    }
    let markdown = markdown_blocks(&page.markdown);
    if !markdown.is_empty() {
        return markdown;
    }
    if !page.text.trim().is_empty() {
        return vec![Block {
            kind: BlockKind::Body,
            text: page.text.clone(),
        }];
    }
    Vec::new()
}

fn layout_block_to_block(block: &LayoutBlock) -> Option<Block> {
    match block.kind {
        // Floaters (figures, rules) contribute no text blocks of their own.
        "figure" | "rule" => None,
        "heading" => {
            let text = block.text.as_deref().unwrap_or_default().trim().to_string();
            if text.is_empty() {
                return None;
            }
            Some(Block {
                kind: BlockKind::Heading(block.level.unwrap_or(1).min(6)),
                text,
            })
        }
        _ => {
            let text = layout_block_text(block);
            if text.trim().is_empty() {
                None
            } else {
                Some(Block {
                    kind: BlockKind::Body,
                    text,
                })
            }
        }
    }
}

/// Render a non-heading layout block as flat body text (tables are laid out
/// one row per line, cells tab-separated, so they survive downstream
/// tokenization).
fn layout_block_text(block: &LayoutBlock) -> String {
    match block.kind {
        "table" => {
            let mut out = String::new();
            if let Some(header) = &block.header {
                let row = render_cells(header);
                if !row.is_empty() {
                    out.push_str(&row);
                    out.push('\n');
                }
            }
            if let Some(rows) = &block.rows {
                for row in rows {
                    let row = render_cells(row);
                    if !row.is_empty() {
                        out.push_str(&row);
                        out.push('\n');
                    }
                }
            }
            out
        }
        _ => match (&block.text, &block.lines) {
            (Some(text), _) if !text.trim().is_empty() => text.clone(),
            (_, Some(lines)) => lines.join("\n"),
            _ => String::new(),
        },
    }
}

fn render_cells(cells: &[LayoutCell]) -> String {
    cells
        .iter()
        .map(|cell| cell.text.trim())
        .filter(|cell| !cell.is_empty())
        .collect::<Vec<_>>()
        .join("\t")
}

/// Split markdown into blocks on `#` headings. Shared by the office parser
/// (which gets markdown from `office_oxide`) and the fallback for liteparse
/// pages without a layout decomposition.
fn markdown_blocks(markdown: &str) -> Vec<Block> {
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

    blocks
}

/// Convert a HEIC/HEIF photo to JPEG with macOS's bundled `sips` tool and
/// return the temporary output path (kept alive for the duration of the parse;
/// the OS temp dir is cleaned up periodically).
fn heic_to_jpeg(path: &str) -> Result<String, String> {
    let out = std::env::temp_dir()
        .join(format!("worxheet_{}.jpg", ulid::Ulid::new()))
        .to_string_lossy()
        .to_string();
    let status = std::process::Command::new("sips")
        .arg("-s")
        .arg("format")
        .arg("jpeg")
        .arg("-s")
        .arg("formatOptions")
        .arg("85")
        .arg(path)
        .arg("--out")
        .arg(&out)
        .status()
        .map_err(|e| format!("Failed to run sips (HEIC photos require macOS): {e}"))?;
    if !status.success() {
        return Err(format!("sips failed to convert HEIC photo: {path}"));
    }
    Ok(out)
}

/// Office formats: markdown export preserves headings (`#`) and slide titles;
/// split on those markers into structured blocks.
fn parse_office_blocks(path: &str) -> Result<Vec<Block>, String> {
    let markdown =
        office_oxide::to_markdown(path).map_err(|e| format!("Failed to extract text: {e}"))?;

    let mut blocks = markdown_blocks(&markdown);

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

#[allow(clippy::too_many_arguments)]
pub async fn process_files(
    pool: &sqlx::SqlitePool,
    worksheet_id: &str,
    file_ids: &[String],
    start_position: i32,
    tokenizer: Arc<Tokenizer>,
    mut on_progress: Option<Box<dyn FnMut(usize, usize) + Send>>,
    logs: Option<Arc<RunLogs>>,
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> Result<Vec<Segment>, String> {
    let total_files = file_ids.len();
    if total_files == 0 {
        return Ok(Vec::new());
    }
    if super::is_cancel_requested(&cancel) {
        return Err(String::from(super::CANCELLED_MESSAGE));
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
                    // `on_page` fires from the parse worker thread, so it must
                    // be Sync: an atomic page count + a channel sender suffice.
                    let on_parse = |done: usize, total: usize| {
                        pages.store(total, Ordering::SeqCst);
                        let _ = page_sender.blocking_send(Msg::Progress(done, total));
                    };
                    let blocks = parse_blocks(&path, &extension, Some(&on_parse))?;
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
            tokio::select! {
                biased;
                _ = cancel_requested(&cancel) => {
                    return Err(String::from(super::CANCELLED_MESSAGE));
                }
                msg = rx.recv() => {
                    match msg.ok_or_else(|| String::from("Parse channel closed unexpectedly"))? {
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
            }
        }
        if super::is_cancel_requested(&cancel) {
            return Err(String::from(super::CANCELLED_MESSAGE));
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

    /// Hand-build a minimal one-page PDF: a large bold heading line followed by
    /// two regular body lines. PDFium parses it and liteparse's layout
    /// classifier should treat the big text as a heading.
    fn make_test_pdf() -> Vec<u8> {
        fn obj(out: &mut Vec<u8>, number: u32, body: &[u8]) -> usize {
            let offset = out.len();
            out.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
            offset
        }

        let mut out = Vec::new();
        out.extend_from_slice(b"%PDF-1.4\n");

        let content = b"BT /F1 24 Tf 72 700 Td (Introduction) Tj ET\n\
BT /F2 12 Tf 72 672 Td (This is a body paragraph with several words for the section.) Tj ET\n\
BT /F2 12 Tf 72 654 Td (A second body sentence keeps the section flowing onward.) Tj ET\n";
        let mut stream = Vec::new();
        stream.extend_from_slice(format!("<< /Length {} >>\nstream\n", content.len()).as_bytes());
        stream.extend_from_slice(content);
        stream.extend_from_slice(b"\nendstream");

        // Object 1 = catalog, 2 = pages, 3 = page, 4 = bold font, 5 = regular,
        // 6 = content stream.
        let offsets = vec![
            obj(&mut out, 1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            obj(&mut out, 2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            obj(
                &mut out,
                3,
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
/Resources << /Font << /F1 4 0 R /F2 5 0 R >> >> /Contents 6 0 R >>",
            ),
            obj(
                &mut out,
                4,
                b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>",
            ),
            obj(
                &mut out,
                5,
                b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
            ),
            obj(&mut out, 6, &stream),
        ];

        let xref_offset = out.len();
        let count = offsets.len() + 1;
        out.extend_from_slice(format!("xref\n0 {count}\n").as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size {count} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n")
                .as_bytes(),
        );
        out
    }

    // Verified: a hand-rolled PDF that PDFium accepts, with a large bold heading
    // line that liteparse's classifier must surface as a Heading block.
    #[test]
    fn test_parse_pdf_extracts_heading_and_body() {
        let pdf = make_test_pdf();
        let path = std::env::temp_dir().join(format!("worxheet_pdf_{}.pdf", ulid::Ulid::new()));
        std::fs::write(&path, &pdf).unwrap();
        let result = parse_blocks(&path.to_string_lossy(), "pdf", None);
        let _ = std::fs::remove_file(&path);
        let blocks = result.unwrap();

        assert!(blocks.iter().any(|b| b.text.contains("Introduction")));
        assert!(
            blocks.iter().any(|b| matches!(b.kind, BlockKind::Heading(_))),
            "layout classifier should mark the intro as a heading"
        );
        let body = blocks
            .iter()
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(body.contains("body paragraph"));
    }

    #[test]
    fn test_markdown_blocks_splits_headings() {
        let blocks = markdown_blocks("# Chapter 1\nSome body text.\n\n## 1.1 Sub\nMore text.\n");
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks[0], Block {
            kind: BlockKind::Heading(1),
            text: "Chapter 1".to_string(),
        });
        assert_eq!(blocks[1], Block {
            kind: BlockKind::Body,
            text: "Some body text.\n\n".to_string(),
        });
        assert_eq!(blocks[2], Block {
            kind: BlockKind::Heading(2),
            text: "1.1 Sub".to_string(),
        });
        assert!(blocks[3].text.contains("More text."));
    }

    #[test]
    fn test_layout_blocks_map_to_headings_and_body() {
        let heading = layout_block("heading");
        assert_eq!(layout_block_to_block(&heading), Some(Block {
            kind: BlockKind::Heading(3),
            text: "Chapter 2".to_string(),
        }));

        assert_eq!(layout_block_to_block(&LayoutBlock {
            kind: "figure",
            text: None,
            level: None,
            bold: false,
            italic: false,
            ordered: None,
            marker: None,
            lines: None,
            lang: None,
            header: None,
            rows: None,
            id: None,
            format: None,
            bbox: None,
        }), None);

        let paragraph = layout_block("paragraph");
        assert_eq!(layout_block_to_block(&paragraph), Some(Block {
            kind: BlockKind::Body,
            text: "Plain body words here.".to_string(),
        }));
    }

    /// A `LayoutBlock` for the given kind with representative content.
    fn layout_block(kind: &'static str) -> LayoutBlock {
        let (text, level) = match kind {
            "heading" => (Some("Chapter 2".to_string()), Some(3)),
            "table" => (None, None),
            _ => (Some("Plain body words here.".to_string()), None),
        };
        LayoutBlock {
            kind,
            text,
            level,
            bold: false,
            italic: false,
            ordered: None,
            marker: None,
            lines: None,
            lang: None,
            header: None,
            rows: None,
            id: None,
            format: None,
            bbox: None,
        }
    }

    #[test]
    fn test_layout_table_renders_rows() {
        let table = LayoutBlock {
            kind: "table",
            text: None,
            level: None,
            bold: false,
            italic: false,
            ordered: None,
            marker: None,
            lines: None,
            lang: None,
            header: Some(vec![
                LayoutCell { text: "Name".to_string(), bbox: None },
                LayoutCell { text: "Value".to_string(), bbox: None },
            ]),
            rows: Some(vec![
                vec![
                    LayoutCell { text: "A".to_string(), bbox: None },
                    LayoutCell { text: "1".to_string(), bbox: None },
                ],
                vec![
                    LayoutCell { text: "B".to_string(), bbox: None },
                    LayoutCell { text: "2".to_string(), bbox: None },
                ],
            ]),
            id: None,
            format: None,
            bbox: None,
        };
        let text = layout_block_text(&table);
        assert!(text.starts_with("Name\tValue\n"));
        assert!(text.contains("A\t1\n"));
        assert!(text.contains("B\t2\n"));
    }

    /// A complexity-stats stub carrying only the fields the OCR gate reads.
    fn complexity_stats(reasons: Vec<ComplexityReason>) -> PageComplexityStats {
        PageComplexityStats {
            page_number: 1,
            text_length: 0,
            text_coverage: 0.0,
            has_substantial_images: reasons.contains(&ComplexityReason::EmbeddedImages),
            image_block_count: 0,
            image_coverage: 0.0,
            largest_image_coverage: 0.0,
            full_page_image: false,
            uncovered_vector_area: None,
            is_garbled: false,
            page_area: 0.0,
            needs_ocr: !reasons.is_empty(),
            reasons,
            layout: None,
        }
    }

    #[test]
    fn test_page_needs_ocr_ignores_figure_pages() {
        // A born-digital textbook page with an inline figure is flagged
        // `EmbeddedImages` but its text layer is complete: no OCR.
        assert!(!page_needs_ocr(&complexity_stats(vec![
            ComplexityReason::EmbeddedImages
        ])));
        // A thin-but-readable page (`SparseText`) is likewise text-first.
        assert!(!page_needs_ocr(&complexity_stats(vec![
            ComplexityReason::EmbeddedImages,
            ComplexityReason::SparseText,
        ])));
        // Scanned / textless / garbled pages genuinely need OCR.
        assert!(page_needs_ocr(&complexity_stats(vec![
            ComplexityReason::Scanned,
            ComplexityReason::EmbeddedImages,
        ])));
        assert!(page_needs_ocr(&complexity_stats(vec![
            ComplexityReason::Garbled
        ])));
        assert!(page_needs_ocr(&complexity_stats(vec![
            ComplexityReason::NoText
        ])));
    }

    #[test]
    fn test_ocr_gate_enables_ocr_only_when_needed_share_is_large() {
        // Figure-heavy born-digital book: every page only `EmbeddedImages` →
        // well below the threshold, OCR stays off.
        let born_digital: Vec<PageComplexityStats> = (0..8)
            .map(|_| complexity_stats(vec![ComplexityReason::EmbeddedImages]))
            .collect();
        assert!(ocr_needed_page_fraction(&born_digital) < OCR_NEEDED_PAGE_FRACTION);

        // A genuinely scanned stack: most pages carry a full-page raster with
        // no text → at/over the threshold, OCR engages.
        let scans: Vec<PageComplexityStats> = (0..8)
            .map(|i| {
                complexity_stats(if i < 5 {
                    vec![ComplexityReason::Scanned]
                } else {
                    vec![ComplexityReason::EmbeddedImages]
                })
            })
            .collect();
        assert!(ocr_needed_page_fraction(&scans) >= OCR_NEEDED_PAGE_FRACTION);

        // Empty/unreadable probe → `is_complex` returned nothing: no OCR.
        assert_eq!(ocr_needed_page_fraction(&[]), 0.0);
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
