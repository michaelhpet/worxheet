# Dependencies

## Rust Crates (core/Cargo.toml)

| Dependency | Purpose | Rationale |
|---|---|---|
| `tauri` 2.x | Desktop app framework | Cross-platform native shell with webview UI. Provides IPC, window management, and system integration. |
| `tauri-plugin-dialog` | Native file picker | Opens the OS file dialog for selecting PDFs, slides, and documents. |
| `tauri-plugin-fs` | File system access | Reads selected files for parsing and metadata extraction. |
| `tauri-plugin-opener` | External file opening | Opens exported artifacts in the default OS application. |
| `sqlx` (sqlite + migrate + time) | Database driver | Async SQLite with compile-time query checking. Manages schema migrations, connection pooling, and type-safe queries. |
| `ulid` | ID generation | Universally Unique Lexicographically Sortable Identifiers for worksheet and file records. Sorted nature matches SQLite B-tree ordering. |
| `time` | Date/time handling | RFC 3339 serialization for SQLite timestamp columns. |
| `serde` + `serde_json` | Serialization | Required by Tauri for IPC payloads and by sqlx for row mapping. |
| `pdf_oxide` | PDF text extraction | Pure Rust PDF parser. 100% pass rate on academic PDFs including multi-column layouts, embedded fonts, and Unicode. Outputs structured Markdown. |
| `office_oxide` | Office document extraction | Pure Rust parser for PPTX, DOCX, PPT, DOC, XLSX, XLS. Supports all common educational document formats with a single crate. |
| `mistralrs` | Local LLM inference | Pure Rust (candle-based) runtime for GGUF models. Supports both text generation and embedding extraction through a single API. Zero C++ dependencies. |
| `candle` | ML framework (via mistralrs) | Pure Rust tensor library. Backend for mistralrs. Compiles with only `rustc`, no C++ toolchain required. |

AI Models (downloaded on first launch)

| Model | Format | Size | Purpose |
|---|---|---|---|
| `bge-small-en-v1.5` Q4_K_M | GGUF | ~24 MB | Text embedding. 384-dimensional vectors. Used to embed document chunks for semantic search. |
| `SmolLM2-360M-Instruct` Q4_K_M | GGUF | ~271 MB | Text generation. Used for few-shot generation of MCQs, summaries, and mind-map structures. 8K context window, best-in-class instruction following for its size. |

## Frontend Dependencies (package.json)

| Dependency | Purpose |
|---|---|
| `@tanstack/react-router` | File-based client-side routing with type-safe navigation |
| `@tanstack/react-query` | Server state management for IPC command results |
| `@tauri-apps/api` | Tauri IPC bridge (invoke Rust commands, listen to events) |
| `@tauri-apps/plugin-dialog` | Frontend binding for the native file dialog |
| `@tauri-apps/plugin-fs` | Frontend binding for filesystem reads |
| `tailwindcss` 4 | Utility-first CSS framework |
| `@base-ui/react` | Headless UI primitives (button, separator, checkbox) |
| `class-variance-authority` | Component variant API (used by shadcn components) |
| `clsx` + `tailwind-merge` | Conditional class merging |

## Excluded for MVP

- **Video transcription**: Requires ffmpeg sidecar (~8-40 MB) + Whisper model (~31-57 MB). Will be added in a future phase.
- **OCR / handwriting recognition**: Requires Tesseract or a vision model. Handwritten content is out of scope for the initial release.
- **Image extraction from slides**: Diagrams and embedded images in slides/documents are not processed. Only text content is extracted and indexed.
