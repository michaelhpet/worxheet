# Roadmap

## Phase 1 — MVP

Core RAG pipeline for text-based documents.

- [x] Tauri 2 project scaffold
- [x] SQLite database setup with migrations
- [x] File picker and metadata display (home screen UI)
- [x] Worksheet CRUD (create, list)
- [ ] PDF text extraction via `pdf_oxide`
- [ ] PPTX / DOCX text extraction via `office_oxide`
- [ ] Text chunking (recursive split, ~512 tokens)
- [ ] Embedding pipeline (`bge-small-en-v1.5` via `mistralrs`)
- [ ] Embedding storage as SQLite BLOBs
- [ ] RAG retrieval (cosine similarity in Rust)
- [ ] HOTS generation (`SmolLM2-360M-Instruct` via `mistralrs`)
- [ ] Artifact persistence (MCQ items, summaries, mind-maps in SQLite)
- [ ] React workspace view for generated artifacts
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
