# Roadmap

Status legend: `[x]` done · `[ ]` not started.

## Phase 1 — MVP

Core RAG pipeline for text-based documents.

- [x] Tauri 2 project scaffold
- [x] SQLite database setup with migrations
- [x] File picker and metadata display (home screen UI)
- [x] Worksheet CRUD (create, list, delete)
- [x] PDF text extraction via `pdf_oxide`
- [x] PPTX / DOCX text extraction via `office_oxide`
- [x] Text chunking (recursive split, 512 tokens / 128 overlap)
- [x] Parse → chunk → store pipeline (`pipeline/ingest.rs`, wired into SQLite)
- [x] Model auto-download via `hf-hub` (Q8_0 GGUFs + `tokenizer.json`, `models.rs`; lazy `ModelPool`)
- [x] Embedding pipeline (`bge-small-en-v1.5` via `llama-cpp-2`, `pipeline/embed.rs`)
- [x] Embedding storage as SQLite BLOBs (`chunks.embedding`)
- [x] HOTS generation (`SmolLM2-360M-Instruct` via `llama-cpp-2`, grammar-constrained JSON for all 5 artifact types, `pipeline/generate.rs`)
- [x] Clustering / topic sampling for multi-artifact generation (HDBSCAN via `hdbscan-rs`; `process_files` persists clusters, `generate_artifacts` exhausts the material per cluster)
- [x] Artifact persistence (MCQ items, summaries, mind-maps in SQLite `artifacts` table)
- [x] React workspace view for generated artifacts (worksheet detail route: status banner + artifact cards)
- [x] Automatic background pipeline (`pipeline/jobs.rs`: `create_worksheet` starts the job, `pipeline-progress` events + `get_pipeline_status`, `resume_stale` on startup)
- [x] Parallel file parsing/chunking in `ingest.rs` (bounded worker waves sized to available parallelism)
- [x] Quiz setup tabs on the worksheet detail page (question count bounded by generated artifacts, optional timer with per-type multipliers)
- [x] Dedicated quiz routes (`/mcq`, `/essay`, `/completion`) sharing a custom quiz shell: step wizard, countdown timer with auto-submit, submit/leave confirmation dialogs
- [x] Client-side grading and results view (percentage + raw score, per-question breakdown, essay self-review against model answers)

## Phase 1.5 — Cloud Generation Pivot (done)

- [x] Drop local llama.cpp inference; generation via OpenAI-compatible providers (`provider/`: OpenAI, Gemini compat, Ollama, LM Studio, custom base URL)
- [x] BYO API key stored in the OS keychain; Settings screen with presets, model listing, and connection test
- [x] Embedder swap: `nomic-embed-text-v1.5` int8 ONNX via `ort`, vendored tokenizer, one-time ~137MB download
- [x] Structure-aware parsing (PDF font-stat headings, Office markdown headings) feeding a hybrid segmenter (structure → embedding-drift → windows)
- [x] Remove HDBSCAN clustering stack (`hdbscan-rs`, `linfa`, `ndarray`); exhaustive contiguous segments replace cluster sampling
- [x] Parallel bulk generation: semaphore fan-out across all artifact types with request/token telemetry
- [x] Deterministic validation layer: option normalization/uniqueness, answer-in-options, grounding ratio, figure-reference rejection, cross-unit dedup
- [x] Hardened prompts: fixed item counts, media/meta-reference bans, formatting contract, MCQ exemplar

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
