//! File parsing + segmentation + persistence.

use std::sync::Arc;

use liteparse::layout::{LayoutBlock, LayoutCell};
use liteparse::ocr_merge::{ComplexityReason, PageComplexityStats};
use liteparse::types::PdfInput;
use liteparse::{LiteParse, LiteParseConfig, OutputFormat, ParsedPage, DEFAULT_PAGE_BATCH_SIZE};
use tokenizers::Tokenizer;
use tokio::sync::mpsc;

use crate::logging::{self, RunLogs};
use crate::schema::Segment;

use super::segment::{self, Block, BlockKind, SegmentDraft};
use super::{PipelineError, PipelineResult};

const DOCUMENT_EXTENSIONS: &[&str] = &[
    "pdf", "jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp", "svg",
];
/// Plain-text sources read straight off disk (no OCR, no office export).
const TEXT_EXTENSIONS: &[&str] = &["txt", "md", "csv"];

/// OCR dominates parse cost, so PDFs only enable it past this share of
/// genuinely text-less pages.
const OCR_NEEDED_PAGE_FRACTION: f32 = 0.5;

/// Parse a file into ordered typed blocks.
pub fn parse_blocks(path: &str, extension: &str) -> PipelineResult<Vec<Block>> {
    let ext = extension.to_lowercase();
    if DOCUMENT_EXTENSIONS.contains(&ext.as_str()) {
        return liteparse_blocks(path, ext == "pdf");
    }
    if TEXT_EXTENSIONS.contains(&ext.as_str()) {
        return parse_text_blocks(path, &ext);
    }
    match ext.as_str() {
        "pptx" | "docx" | "ppt" | "doc" => parse_office_blocks(path),
        _ => Err(PipelineError::Failed(format!(
            "Unsupported file extension: {extension}"
        ))),
    }
}

/// Plain UTF-8 read for txt/md/csv. Markdown reuses heading splitting so
/// sections survive; txt/csv become a single body block (csv kept raw).
fn parse_text_blocks(path: &str, extension: &str) -> PipelineResult<Vec<Block>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| PipelineError::Failed(format!("Failed to read text file {path}: {e}")))?;
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    if extension == "md" {
        return Ok(markdown_blocks(&text));
    }
    Ok(vec![Block {
        kind: BlockKind::Body,
        text,
    }])
}

/// Pages stream in bounded batches so a large document never sits fully in memory.
fn liteparse_blocks(path: &str, pdf: bool) -> PipelineResult<Vec<Block>> {
    // Bundled Tesseract unless `WORXHEET_OCR_SERVER_URL` points at an OCR sidecar.
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

    let mut session = tauri::async_runtime::block_on(
        parser.open_batch_session(PdfInput::Path(path.to_string()), DEFAULT_PAGE_BATCH_SIZE),
    )
    .map_err(|e| PipelineError::Failed(format!("Failed to open document {path}: {e}")))?;

    let mut blocks = Vec::new();
    while let Some(batch) = tauri::async_runtime::block_on(session.next_batch())
        .map_err(|e| PipelineError::Failed(format!("Failed to parse document {path}: {e}")))?
    {
        for page in &batch.result.pages {
            blocks.extend(blocks_from_page(page));
        }
    }
    Ok(blocks)
}

/// Image-only flags (`EmbeddedImages`) and thin-but-readable text
/// (`SparseText`) still yield native text, so they must not count toward OCR.
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

fn ocr_needed_page_fraction(stats: &[PageComplexityStats]) -> f32 {
    if stats.is_empty() {
        return 0.0;
    }
    stats.iter().filter(|s| page_needs_ocr(s)).count() as f32 / stats.len() as f32
}

/// Cheap pre-OCR pass, no rendering. Fails open so nothing silently loses content.
fn ocr_enabled_for_pdf(parser: &LiteParse, path: &str) -> bool {
    let stats = tauri::async_runtime::block_on(parser.is_complex(PdfInput::Path(path.to_string())));
    match stats {
        Ok(stats) => ocr_needed_page_fraction(&stats) >= OCR_NEEDED_PAGE_FRACTION,
        Err(_) => true,
    }
}

/// Layout first, then markdown, then raw text.
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

/// Tables render one row per line, cells tab-separated.
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

/// Split markdown into blocks on `#` headings.
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

fn parse_office_blocks(path: &str) -> PipelineResult<Vec<Block>> {
    let markdown = office_oxide::to_markdown(path)
        .map_err(|e| PipelineError::Failed(format!("Failed to export office markdown: {e}")))?;

    let mut blocks = markdown_blocks(&markdown);

    if blocks.is_empty() {
        let plain = office_oxide::extract_text(path)
            .map_err(|e| PipelineError::Failed(format!("Failed to extract office text: {e}")))?;
        if !plain.trim().is_empty() {
            blocks.push(Block {
                kind: BlockKind::Body,
                text: plain,
            });
        }
    }

    Ok(blocks)
}

pub async fn unchunked_files(
    pool: &sqlx::SqlitePool,
    worksheet_id: &str,
    file_ids: &[String],
) -> PipelineResult<Vec<String>> {
    if file_ids.is_empty() {
        return Ok(Vec::new());
    }

    let chunked: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT file_id FROM chunks WHERE worksheet_id = ?")
            .bind(worksheet_id)
            .fetch_all(pool)
            .await?;

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
) -> PipelineResult<i32> {
    let (max_position,): (Option<i32>,) =
        sqlx::query_as("SELECT MAX(position) FROM chunks WHERE worksheet_id = ?")
            .bind(worksheet_id)
            .fetch_one(pool)
            .await?;

    Ok(max_position.unwrap_or(-1) + 1)
}

/// Copy chunks from same-content files in other worksheets instead of
/// re-parsing duplicates. Returns files still needing a real parse.
pub async fn reuse_chunks(
    pool: &sqlx::SqlitePool,
    worksheet_id: &str,
    file_ids: &[String],
    start_position: i32,
) -> PipelineResult<(Vec<String>, i32)> {
    if file_ids.is_empty() {
        return Ok((Vec::new(), start_position));
    }

    let mut builder = sqlx::QueryBuilder::new(
        "SELECT id, identity_key FROM files WHERE worksheet_id = ",
    );
    builder.push_bind(worksheet_id);
    builder.push(" AND id IN (");
    {
        let mut separated = builder.separated(", ");
        for id in file_ids {
            separated.push_bind(id);
        }
    }
    builder.push(")");
    let identity_rows: Vec<(String, Option<String>)> =
        builder.build_query_as().fetch_all(pool).await?;

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

        let source: Option<(String,)> = sqlx::query_as(
            "SELECT c.file_id
             FROM chunks c
             JOIN files f ON f.id = c.file_id
             WHERE f.identity_key = ? AND c.worksheet_id <> ? AND c.worksheet_id IS NOT NULL
             GROUP BY c.file_id
             ORDER BY MIN(c.position)
             LIMIT 1",
        )
        .bind(&identity)
        .bind(worksheet_id)
        .fetch_optional(pool)
        .await?;

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
        .await?;

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
        let mut transaction = pool.begin().await?;

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
            builder.build().execute(&mut *transaction).await?;
        }

        transaction.commit().await?;
    }

    sqlx::query("UPDATE worksheets SET updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(worksheet_id)
        .execute(pool)
        .await?;

    Ok((remaining, position))
}

pub async fn process_files(
    pool: &sqlx::SqlitePool,
    worksheet_id: &str,
    file_ids: &[String],
    start_position: i32,
    tokenizer: Arc<Tokenizer>,
    logs: Option<Arc<RunLogs>>,
    stop: super::Stop,
) -> PipelineResult<Vec<Segment>> {
    let total_files = file_ids.len();
    if total_files == 0 {
        return Ok(Vec::new());
    }
    stop.check()?;

    let mut metadata = Vec::with_capacity(total_files);
    for file_id in file_ids {
        let row = sqlx::query_as::<_, (String, String)>(
            "SELECT path, extension FROM files WHERE id = ? AND worksheet_id = ?",
        )
        .bind(file_id)
        .bind(worksheet_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| PipelineError::Failed(format!("Failed to query file {file_id}: {e}")))?
        .ok_or_else(|| PipelineError::Failed(format!("File not found: {file_id}")))?;
        metadata.push((file_id.clone(), row.0, row.1));
    }

    let max_parallel = total_files.min(
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4),
    );

    let mut segmented: Vec<Option<Vec<SegmentDraft>>> = vec![None; total_files];

    let (tx, mut rx) =
        mpsc::channel::<(usize, PipelineResult<Vec<SegmentDraft>>)>(total_files.max(1));

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
                let result = (|| -> PipelineResult<Vec<SegmentDraft>> {
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
                            &format!("parse_started_{}", logging::sanitize_label(&file_id)),
                            &started_record,
                        );
                    }
                    let parse_started = std::time::Instant::now();
                    let blocks = parse_blocks(&path, &extension)?;
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
                            "duration_ms": parse_started.elapsed().as_millis(),
                        });
                        logs.write_json("file_reads", &logging::sanitize_label(&file_id), &record);
                    }

                    let seg_started = std::time::Instant::now();
                    let mut trace = segment::TokenizeTrace::default();
                    let drafts = segment::segment_blocks(blocks, &tokenizer, Some(&mut trace));
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

                        let segment_record = serde_json::json!({
                            "timestamp": logging::rfc3339_utc(),
                            "count": drafts.len(),
                            "total_tokens": drafts.iter().map(|d| d.tokens).sum::<usize>(),
                        });
                        logs.write_json(
                            "segmentation",
                            &logging::sanitize_label(&file_id),
                            &segment_record,
                        );
                    }
                    Ok(drafts)
                })();
                let _ = tx.blocking_send((index, result));
            });
        }

        let stop_watch = stop.clone();
        while done < end {
            tokio::select! {
                biased;
                _ = stop_watch.stopped() => {
                    return Err(PipelineError::Cancelled);
                }
                msg = rx.recv() => {
                    let (index, result) =
                        msg.ok_or_else(|| {
                            PipelineError::Failed(String::from("Parse channel closed unexpectedly"))
                        })?;
                    segmented[index] = Some(result?);
                    done += 1;
                }
            }
        }
        stop.check()?;
        start = end;
    }

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

    let mut transaction = pool.begin().await?;

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
        builder.build().execute(&mut *transaction).await?;
    }

    transaction.commit().await?;

    sqlx::query("UPDATE worksheets SET updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(worksheet_id)
        .execute(pool)
        .await?;

    Ok(all_segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

    #[test]
    fn test_parse_unsupported_extension() {
        assert!(parse_blocks("test.mp3", "mp3").is_err());
        assert!(parse_blocks("test.heic", "heic").is_err());
    }

    fn write_temp_file(name: &str, contents: &str) -> String {
        let path = std::env::temp_dir().join(format!(
            "worxheet_{}_{}.{}",
            name,
            ulid::Ulid::new(),
            name.rsplit('.').next().unwrap_or("txt")
        ));
        std::fs::write(&path, contents).unwrap();
        path.to_string_lossy().to_string()
    }

    #[test]
    fn test_parse_txt_becomes_single_body_block() {
        let path = write_temp_file(
            "sample.txt",
            "First line of plain text.\nSecond line follows.\n",
        );
        let blocks = parse_blocks(&path, "txt").unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::Body);
        assert!(blocks[0].text.contains("First line"));
    }

    #[test]
    fn test_parse_md_splits_headings() {
        let path = write_temp_file("sample.md", "# Title\nBody text here.\n\n## Sub\nMore.\n");
        let blocks = parse_blocks(&path, "md").unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(blocks
            .iter()
            .any(|b| matches!(b.kind, BlockKind::Heading(1)) && b.text == "Title"));
    }

    #[test]
    fn test_parse_csv_becomes_single_body_block() {
        let path = write_temp_file("sample.csv", "name,value\nA,1\nB,2\n");
        let blocks = parse_blocks(&path, "csv").unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].text.contains("name,value"));
    }

    #[test]
    fn test_parse_empty_text_yields_no_blocks() {
        let path = write_temp_file("empty.txt", "   \n");
        let blocks = parse_blocks(&path, "txt").unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_parse_missing_file() {
        let path = format!("{}/nonexistent.pdf", FIXTURE_DIR);
        assert!(parse_blocks(&path, "pdf").is_err());
    }

    /// Minimal one-page PDF: one large bold heading line, two body lines.
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

    #[test]
    fn test_parse_pdf_extracts_heading_and_body() {
        let pdf = make_test_pdf();
        let path = std::env::temp_dir().join(format!("worxheet_pdf_{}.pdf", ulid::Ulid::new()));
        std::fs::write(&path, &pdf).unwrap();
        let result = parse_blocks(&path.to_string_lossy(), "pdf");
        let _ = std::fs::remove_file(&path);
        let blocks = result.unwrap();

        assert!(blocks.iter().any(|b| b.text.contains("Introduction")));
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b.kind, BlockKind::Heading(_))),
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
        assert_eq!(
            blocks[0],
            Block {
                kind: BlockKind::Heading(1),
                text: "Chapter 1".to_string(),
            }
        );
        assert_eq!(
            blocks[1],
            Block {
                kind: BlockKind::Body,
                text: "Some body text.\n\n".to_string(),
            }
        );
        assert_eq!(
            blocks[2],
            Block {
                kind: BlockKind::Heading(2),
                text: "1.1 Sub".to_string(),
            }
        );
        assert!(blocks[3].text.contains("More text."));
    }

    #[test]
    fn test_layout_blocks_map_to_headings_and_body() {
        let heading = layout_block("heading");
        assert_eq!(
            layout_block_to_block(&heading),
            Some(Block {
                kind: BlockKind::Heading(3),
                text: "Chapter 2".to_string(),
            })
        );

        assert_eq!(
            layout_block_to_block(&LayoutBlock {
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
            }),
            None
        );

        let paragraph = layout_block("paragraph");
        assert_eq!(
            layout_block_to_block(&paragraph),
            Some(Block {
                kind: BlockKind::Body,
                text: "Plain body words here.".to_string(),
            })
        );
    }

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
                LayoutCell {
                    text: "Name".to_string(),
                    bbox: None,
                },
                LayoutCell {
                    text: "Value".to_string(),
                    bbox: None,
                },
            ]),
            rows: Some(vec![
                vec![
                    LayoutCell {
                        text: "A".to_string(),
                        bbox: None,
                    },
                    LayoutCell {
                        text: "1".to_string(),
                        bbox: None,
                    },
                ],
                vec![
                    LayoutCell {
                        text: "B".to_string(),
                        bbox: None,
                    },
                    LayoutCell {
                        text: "2".to_string(),
                        bbox: None,
                    },
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
        assert!(!page_needs_ocr(&complexity_stats(vec![
            ComplexityReason::EmbeddedImages
        ])));
        assert!(!page_needs_ocr(&complexity_stats(vec![
            ComplexityReason::EmbeddedImages,
            ComplexityReason::SparseText,
        ])));
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
        let born_digital: Vec<PageComplexityStats> = (0..8)
            .map(|_| complexity_stats(vec![ComplexityReason::EmbeddedImages]))
            .collect();
        assert!(ocr_needed_page_fraction(&born_digital) < OCR_NEEDED_PAGE_FRACTION);

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
        identity_key: Option<&str>,
    ) -> String {
        let id = ulid::Ulid::new().to_string();
        sqlx::query(
            "INSERT INTO files (id, worksheet_id, path, name, extension, size, identity_key)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(worksheet_id)
        .bind("/tmp/sample.pdf")
        .bind("sample.pdf")
        .bind("pdf")
        .bind(10i64)
        .bind(identity_key)
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
