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
Upload ─► Parse ─► Chunk ─► Store ─► Embed ─► Retrieve ─► Generate ─► Persist
```
All stages are wired into Tauri commands and SQLite. Clustering/topic-sampling for
multi-artifact generation remains planned; for now `generate_artifacts` retrieves the
top-k chunks most similar to the artifact task.

1. **Upload** *(done)*: User creates a worksheet and selects files (PDF, PPTX, DOCX) via the Tauri dialog plugin. `create_worksheet` registers the files in SQLite.
2. **Parse** *(done)*: `ingest.rs` extracts text using `pdf_oxide` (PDF) or `office_oxide` (PPTX/DOCX/PPT/DOC), then updates `files.status` (`uploaded → parsing → parsed`).
3. **Chunk** *(done)*: Extracted text is split into overlapping chunks of ~512 tokens with 128-token overlap via `chunk_text` (HF `tokenizers`).
4. **Store** *(done)*: `process_files` inserts chunks into the `chunks` table (text + position + file/worksheet reference).
5. **Embed** *(done)*: `embed_worksheet` embeds chunks without an embedding via `bge-small-en-v1.5` (Q8_0, 384-dim, L2-normalized) and stores the vectors in the `chunks.embedding` BLOB.
6. **Retrieve** *(done)*: `retrieve_chunks` embeds a query and returns the top-k chunks by cosine similarity over the BLOBs (`retrieval.rs`).
7. **Generate** *(done)*: `generate_artifacts` auto-embeds any un-embedded chunks, retrieves top-k context, builds a chat-template prompt, and generates an artifact constrained to a JSON schema via a GBNF grammar (`json_schema_to_grammar`). All five artifact types (`MultipleChoiceQuiz`, `EssayQuiz`, `CompletionQuiz`, `Summary`, `MindMap`) are supported.
8. **Persist** *(done)*: Generated artifacts are validated, inserted into the `artifacts` table, and returned to the frontend via `get_artifacts`.

The React worksheet detail route (`app/routes/worksheets.$id.tsx`) is the user-facing workspace: a files panel (parse status badges, "Process files" with live `ingestion-progress`), and an artifacts panel (type-filtered cards, a generate dialog with count + advanced sampling params, and live `generation-progress` while artifacts are produced).

## Inference

Both models run in the single llama.cpp backend created once per process.

- `llm.rs` holds a process-wide `LlamaBackend` in a `OnceLock`. `llama_backend_init` may only run once, so models and contexts are created from a `&'static LlamaBackend`.
- `models.rs` exposes a `ModelPool` (kept in Tauri state as `Arc<ModelPool>`). It lazily downloads and loads three artifacts on first use: the embedding GGUF, the generation GGUF, and the generation model's `tokenizer.json` (used for chunking).
- Embedding model: `bge-small-en-v1.5` (Q8_0 GGUF, 384-dim).
- Generation model: `SmolLM2-360M-Instruct` (Q8_0 GGUF, 8K context).
- All three artifacts are downloaded from Hugging Face on first use via `hf-hub` and stored in the app data directory (`models_dir`); tests override the location with `WORXHEET_MODELS_DIR`.
- Generation samples under a GBNF grammar chain `[grammar, temp?, top_p, dist(seed)]` using the apply-sampler path (`LlamaTokenDataArray::from_iter(ctx.get_logits_ith(idx))` + `apply_sampler` + `selected_token`) rather than `sampler.sample(ctx, idx)`, which crashes with grammar chains in llama-cpp-2 (see the "Grammar-constrained generation" decision below).

## Directory Layout

```
worxheet/
├── app/                          # React frontend
│   ├── components/               # UI components (workspace/, create-worksheet-dialog, files-uploader, ui/…)
│   ├── data/                     # TanStack Query hooks + IPC wrappers (worksheets, files, artifacts, progress events)
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
│   │   ├── models.rs             # GGUF model paths + hf-hub download + lazy ModelPool
│   │   ├── file.rs               # File metadata command
│   │   ├── worksheet.rs          # Worksheet CRUD commands
│   │   ├── chunk.rs              # Recursive text splitting
│   │   ├── ingest.rs             # Parse + chunk + store pipeline
│   │   ├── embed.rs              # bge-small embedding module
│   │   ├── retrieval.rs          # Embedding BLOB encode/decode + cosine retrieval
│   │   ├── generation.rs         # SmolLM2 grammar-constrained generation
│   │   ├── pipeline.rs           # End-to-end commands (process_files, embed_worksheet, retrieve_chunks, generate_artifacts, …)
│   │   ├── schema.rs             # Shared structs (Chunk, Artifact, ArtifactType, …)
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
All inference runs on-device via `llama-cpp-2`. No API keys, no third-party model serving. This guarantees privacy and offline operation. Models are downloaded once, lazily on first use, and stored in the app data directory.

### Embeddings stored as SQLite BLOBs instead of a vector database
At the expected scale (<10,000 chunks per worksheet), brute-force cosine similarity over float32 arrays stored in SQLite BLOBs takes under 1ms. No vector DB (LanceDB, Pinecone, etc.) is needed. This avoids adding ~170MB+ of dependencies and keeps the architecture simple.

### Single llama.cpp backend for embeddings and generation
Both `bge-small-en-v1.5` (embedding encoder) and `SmolLM2-360M-Instruct` (text decoder) run through the same llama.cpp backend, initialized once per process (`llm.rs`). Models are separate Q8_0 GGUF files loaded at runtime.

### Grammar-constrained generation
Artifacts are generated as structured JSON by compiling a JSON schema into a GBNF grammar (`json_schema_to_grammar`) and sampling under that grammar with `LlamaSampler::grammar`. This guarantees schema-valid output from the small model without post-hoc parsing fixes.

Sampling uses the apply-sampler API (`LlamaTokenDataArray::from_iter(ctx.get_logits_ith(idx))` → `apply_sampler` → `selected_token`) instead of `sampler.sample(ctx, idx)`, because the single-call `sample` path triggers a `GGML_ASSERT(!stacks.empty())` crash in llama-cpp-2 0.1.145+ (upstream issue `utilityai/llama-cpp-rs#1007`). Generation terminates when the empty grammar stack masks every non-EOG token, forcing an EOG token to be selected.

### Inference via llama-cpp-2 (switched from candle/mistralrs)
The project uses `llama-cpp-2` 0.1.154. This is a change from the original candle-based `mistralrs` plan; the bundled C++ build of llama.cpp requires `cmake`/`clang` at build time (~5 minute compile) in exchange for a battle-tested inference engine and the GBNF grammar sampler.
