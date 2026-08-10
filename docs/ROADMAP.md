# Roadmap

Status legend: `[x]` done · `[~]` module implemented, end-to-end wiring pending · `[ ]` not started.

## Phase 1 — MVP

Core RAG pipeline for text-based documents.

- [x] Tauri 2 project scaffold
- [x] SQLite database setup with migrations
- [x] File picker and metadata display (home screen UI)
- [x] Worksheet CRUD (create, list, delete)
- [x] PDF text extraction via `pdf_oxide`
- [x] PPTX / DOCX text extraction via `office_oxide`
- [x] Text chunking (recursive split, 512 tokens / 128 overlap)
- [x] Parse → chunk → store pipeline (`ingest.rs`, wired into SQLite)
- [x] Model auto-download via `hf-hub` (Q8_0 GGUFs, `models.rs`)
- [~] Embedding module (`bge-small-en-v1.5` via `llama-cpp-2`, `embed.rs`) — built + tested; not yet exposed as a command
- [ ] Embedding storage as SQLite BLOBs (`chunks.embedding` currently `NULL`)
- [ ] RAG retrieval (cosine similarity in Rust over chunk BLOBs)
- [~] HOTS generation module (`SmolLM2-360M-Instruct` via `llama-cpp-2`, grammar-constrained JSON, `generation.rs`) — built + tested; not yet wired to a command, RAG context, or persistence
- [ ] Clustering / topic sampling for multi-artifact generation (k-means or HDBSCAN; `ndarray`/`hdbscan-rs`/`linfa` deps reserved)
- [ ] Artifact persistence (MCQ items, summaries, mind-maps in SQLite `artifacts` table)
- [ ] React workspace view for generated artifacts (worksheet detail route currently a placeholder)
- [ ] Generation progress indicators

## Phase 2 — Document Coverage

- [ ] Video → audio transcription (ffmpeg sidecar + Whisper GGUF)
- [ ] OCR for text in images (Tesseract or `ocrs`)
- [ ] Image/diagram extraction from slides
- [ ] Image/diagram processing and captioning

## Phase 3 — Handwriting & Rich Media

- [ ] Handwriting recognition (TrOCR via candle, or alternative)
- [ ] Mind-map visualization in the frontend
- [ ] Artifact export (PDF, Markdown, Anki deck)

## Not Yet Scheduled

- Cloud sync or worksheet sharing
- Multi-user or collaborative features
- Custom model fine-tuning on user content
