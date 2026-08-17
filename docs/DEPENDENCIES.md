# Dependencies

## Rust Crates (core/Cargo.toml)

| Dependency | Purpose | Rationale |
|---|---|---|
| `tauri` 2.x | Desktop app framework | Cross-platform native shell with webview UI. Provides IPC, window management, and system integration. |
| `tauri-plugin-dialog` | Native file picker | Opens the OS file dialog for selecting PDFs, slides, and documents. |
| `tauri-plugin-fs` | File system access | Reads selected files for parsing and metadata extraction. |
| `tauri-plugin-opener` | External file opening | Opens exported artifacts in the default OS application. |
| `sqlx` (sqlite + migrate + time) | Database driver | Async SQLite. Manages schema migrations, connection pooling, and type-safe queries. |
| `ulid` | ID generation | Universally Unique Lexicographically Sortable Identifiers for worksheet and file records. Sorted nature matches SQLite B-tree ordering. |
| `time` | Date/time handling | RFC 3339 serialization for SQLite timestamp columns. |
| `serde` + `serde_json` | Serialization | Required by Tauri for IPC payloads and by sqlx for row mapping. |
| `pdf_oxide` | PDF text extraction | Pure Rust PDF parser. Extracts text page-by-page from academic PDFs including multi-column layouts and Unicode. |
| `office_oxide` | Office document extraction | Pure Rust parser for PPTX, DOCX, PPT, DOC. |
| `llama-cpp-2` 0.1.154 | Local LLM inference | llama.cpp bindings used for **both** `bge-small-en-v1.5` embeddings and `SmolLM2-360M-Instruct` generation, including the GBNF grammar sampler for structured output. |
| `llama-cpp-sys-2` | FFI layer for llama.cpp | Generated bindings plus the bundled C++ llama.cpp source. Compiled at build time via `cc`/`cmake` (requires `cmake` + a C++ compiler, ~5 min). |
| `tokenizers` 0.21 | Tokenization | Hugging Face tokenizers used by `chunk_text` to size and split document chunks. |
| `encoding_rs` | Text decoding | Decodes generated tokens to UTF-8 in `pipeline/generate.rs`. |
| `hf-hub` 0.4 | Model download | Downloads the GGUF/tokenizer artifacts from Hugging Face on first use (`models.rs`). |
| `ndarray`, `hdbscan-rs`, `linfa`, `linfa-clustering` | Clustering | HDBSCAN topic clustering over chunk embeddings (`pipeline/cluster.rs`). |

## AI Models (downloaded on first use)

| Model | Format | Size | Purpose |
|---|---|---|---|
| `bge-small-en-v1.5` Q8_0 | GGUF | ~27 MB | Text embedding. 384-dimensional vectors, L2-normalized. Repo: `ggml-org/bge-small-en-v1.5-Q8_0-GGUF` |
| `SmolLM2-360M-Instruct` Q8_0 | GGUF | ~365 MB | Text generation. Grammar-constrained JSON artifacts (MCQs, summaries, mind-maps). 8K context window. Repo: `HuggingFaceTB/SmolLM2-360M-Instruct-GGUF` |
| `SmolLM2-360M-Instruct` `tokenizer.json` | JSON | ~2 MB | BPE tokenizer used by `chunk_text` to size and split document chunks. Repo: `HuggingFaceTB/SmolLM2-360M-Instruct` |

Models are stored under the app data directory (`models/`). `models.rs` lazily downloads each artifact on first use (via the `ModelPool`); tests may override the location with the `WORXHEET_MODELS_DIR` environment variable.

## Frontend Dependencies (package.json)

| Dependency | Purpose |
|---|---|
| `@tanstack/react-router` | File-based client-side routing with type-safe navigation |
| `@tanstack/react-query` | Server state management for IPC command results |
| `@tanstack/react-form` | Form state management (used by the create-worksheet dialog) |
| `@tauri-apps/api` | Tauri IPC bridge (invoke Rust commands, listen to events) |
| `@tauri-apps/plugin-dialog` | Frontend binding for the native file dialog |
| `@tauri-apps/plugin-fs` | Frontend binding for filesystem reads |
| `@tauri-apps/plugin-opener` | Frontend binding for opening external files |
| `react` / `react-dom` 19 | UI runtime |
| `tailwindcss` 4 | Utility-first CSS framework |
| `@base-ui/react` | Headless UI primitives (button, dialog, field, progress, etc.) |
| `shadcn` | Component CLI/registry used for the `ui/` components |
| `zod` | Runtime schema validation for forms and IPC payloads |
| `class-variance-authority` | Component variant API (used by shadcn components) |
| `clsx` + `tailwind-merge` | Conditional class merging |
| `@tabler/icons-react` | Icon set |
| `@fontsource-variable/geist` | Bundled Geist variable font |
| `tw-animate-css` | Tailwind v4 animation helpers |

## Excluded for MVP

- **Video transcription**: Requires ffmpeg sidecar (~8-40 MB) + Whisper model (~31-57 MB). Will be added in a future phase.
- **OCR / handwriting recognition**: Requires Tesseract or a vision model. Handwritten content is out of scope for the initial release.
- **Image extraction from slides**: Diagrams and embedded images in slides/documents are not processed. Only text content is extracted and indexed.
