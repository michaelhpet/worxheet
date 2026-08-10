# Architecture

## Overview

Worxheet is a local-first desktop monolith. The entire application — UI, business logic, AI inference, and storage — runs on the user's machine with no cloud dependencies. PDFs, slides, and documents are parsed and indexed locally, and all LLM inference uses bundled GGUF models loaded at runtime.

## System Diagram

```
┌─────────────────────────────────────────────────────┐
│                   Tauri Process                       │
│  ┌─────────────────────┐  ┌───────────────────────┐  │
│  │   Frontend (React)   │  │   Backend (Rust)       │  │
│  │   TanStack Router    │◄─┤   IPC Commands          │  │
│  │   TanStack Query     │──►│   llama-cpp-2           │  │
│  │   Tailwind / shadcn   │  │   pdf_oxide             │  │
│  └─────────────────────┘  │   office_oxide           │  │
│                            │   sqlx (SQLite)          │  │
│                            └──────────┬──────────────┘  │
│                            ┌──────────▼──────────────┐  │
│                            │      SQLite Database      │  │
│                            │  (worksheets, files,       │  │
│                            │   chunks, artifacts)       │  │
│                            └─────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

## Data Flow (RAG Pipeline)

Implementation status of each stage:

```
Upload ─► Parse ─► Chunk ─► Store            [wired into SQLite via ingest.rs]
                                │
                                ├─ Embed      [module built + tested, not yet wired to DB]
                                │
Generate (grammar-constrained) ─┘             [module built + tested, not yet a Tauri command]
Retrieve / RAG                                [planned — not implemented]
Persist artifacts                             [planned — artifacts table exists, no write path yet]
```

1. **Upload** *(done)*: User creates a worksheet and selects files (PDF, PPTX, DOCX) via the Tauri dialog plugin. `create_worksheet` registers the files in SQLite.
2. **Parse** *(done)*: `ingest.rs` extracts text using `pdf_oxide` (PDF) or `office_oxide` (PPTX/DOCX/PPT/DOC), then updates `files.status` (`uploaded → parsing → parsed`).
3. **Chunk** *(done)*: Extracted text is split into overlapping chunks of ~512 tokens with 128-token overlap via `chunk_text` (HF `tokenizers`).
4. **Store** *(done)*: Chunks are inserted into the `chunks` table (text + position + file/worksheet reference). Embeddings are not yet written to the `embedding` BLOB column.
5. **Embed** *(module done)*: `embed.rs` wraps `bge-small-en-v1.5` (Q8_0, 384-dim) via llama.cpp and produces L2-normalized vectors. Storing them per-chunk and wiring the command is pending.
6. **Retrieve** *(planned)*: RAG retrieval via cosine similarity over the chunk BLOBs is designed but not implemented.
7. **Generate** *(module done)*: `generation.rs` wraps `SmolLM2-360M-Instruct` (Q8_0) and generates artifacts constrained to a JSON schema via a GBNF grammar (`json_schema_to_grammar`). It is tested but not yet exposed as a command nor persisted.
8. **Persist** *(planned)*: Generated artifacts should be saved to the `artifacts` table and returned to the React workspace. No write path exists yet.

## Inference

Both models run in the single llama.cpp backend created once per process.

- `llm.rs` holds a process-wide `LlamaBackend` in a `OnceLock`. `llama_backend_init` may only run once, so models and contexts are created from a `&'static LlamaBackend`.
- Embedding model: `bge-small-en-v1.5` (Q8_0 GGUF, 384-dim).
- Generation model: `SmolLM2-360M-Instruct` (Q8_0 GGUF, 8K context).
- Both GGUFs are downloaded from Hugging Face on first launch via `hf-hub` (`models.rs::ensure_downloaded`).

## Directory Layout

```
worxheet/
├── app/                          # React frontend
│   ├── components/               # UI components (create-worksheet-dialog, files-uploader, ui/…)
│   ├── data/                     # TanStack Query hooks + IPC wrappers
│   ├── lib/                      # Types, utils, constants
│   ├── routes/                   # File-based TanStack Router routes
│   ├── index.css                 # Tailwind v4 entry
│   └── main.tsx                  # App entry
├── core/                         # Rust backend
│   ├── migrations/               # SQLite migrations
│   ├── src/
│   │   ├── main.rs               # Tauri entry
│   │   ├── lib.rs                # Plugin registration, AppState, invoke handler
│   │   ├── database.rs           # SQLite connection + migration runner
│   │   ├── llm.rs                # Process-wide llama.cpp backend singleton
│   │   ├── models.rs             # GGUF model paths + hf-hub download
│   │   ├── file.rs               # File metadata command
│   │   ├── worksheet.rs          # Worksheet CRUD commands
│   │   ├── chunk.rs              # Recursive text splitting
│   │   ├── ingest.rs             # Parse + chunk + store pipeline
│   │   ├── embed.rs              # bge-small embedding module
│   │   ├── generation.rs         # SmolLM2 grammar-constrained generation
│   │   └── schema.rs             # Shared structs (Chunk, Artifact, ArtifactType, …)
│   ├── Cargo.toml
│   └── tauri.conf.json
├── docs/                         # Project documentation
├── index.html
├── package.json
├── tsconfig.json
└── vite.config.ts
```

## Key Design Decisions

### Local-first, zero cloud dependencies
All inference runs on-device via `llama-cpp-2`. No API keys, no third-party model serving. This guarantees privacy and offline operation. Models are downloaded once at first launch and stored in the app data directory.

### Embeddings stored as SQLite BLOBs instead of a vector database
At the expected scale (<10,000 chunks per worksheet), brute-force cosine similarity over float32 arrays stored in SQLite BLOBs takes under 1ms. No vector DB (LanceDB, Pinecone, etc.) is needed. This avoids adding ~170MB+ of dependencies and keeps the architecture simple.

### Single llama.cpp backend for embeddings and generation
Both `bge-small-en-v1.5` (embedding encoder) and `SmolLM2-360M-Instruct` (text decoder) run through the same llama.cpp backend, initialized once per process (`llm.rs`). Models are separate Q8_0 GGUF files loaded at runtime.

### Grammar-constrained generation
Artifacts are generated as structured JSON by compiling a JSON schema into a GBNF grammar (`json_schema_to_grammar`) and sampling under that grammar with `LlamaSampler::grammar`. This guarantees schema-valid output from the small model without post-hoc parsing fixes.

### Inference via llama-cpp-2 (switched from candle/mistralrs)
The project uses `llama-cpp-2` 0.1.154. This is a change from the original candle-based `mistralrs` plan; the bundled C++ build of llama.cpp requires `cmake`/`clang` at build time (~5 minute compile) in exchange for a battle-tested inference engine and the GBNF grammar sampler.
