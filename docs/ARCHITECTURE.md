# Architecture

## Overview

Worxheet is a local-first desktop monolith. The entire application — UI, business logic, AI inference, and storage — runs on the user's machine with no cloud dependencies. PDFs, slides, and documents are parsed and indexed locally, and all LLM inference uses bundled models loaded at runtime.

## System Diagram

```
┌─────────────────────────────────────────────────────┐
│                   Tauri Process                       │
│  ┌─────────────────────┐  ┌───────────────────────┐  │
│  │   Frontend (React)   │  │   Backend (Rust)       │  │
│  │   TanStack Router    │◄─┤   IPC Commands          │  │
│  │   TanStack Query     │──►│   mistralrs + candle    │  │
│  │   Tailwind / shadcn   │  │   pdf_oxide             │  │
│  └─────────────────────┘  │   office_oxide           │  │
│                            │   sqlx (SQLite)          │  │
│                            └──────────┬──────────────┘  │
│                                       │                  │
│                            ┌──────────▼──────────────┐  │
│                            │      SQLite Database      │  │
│                            │  (worksheets, files,       │  │
│                            │   chunks, embeddings,      │  │
│                            │   artifacts)               │  │
│                            └─────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

## Data Flow (RAG Pipeline)

```
Upload Files ──► Parse ──► Chunk ──► Embed ──► Store ──► Retrieve ──► Generate ──► Persist
                  │          │         │          │            │             │
                  ▼          ▼         ▼          ▼            ▼             ▼
            pdf_oxide    Recursive   bge-small   SQLite      Cosine       SmolLM2-360M
            office_oxide  splitter   via          BLOBs      similarity    via
                         (512 tok,   mistralrs                over BLOBs   mistralrs
                          128 overlap)
```

1. **Upload**: User creates a worksheet and selects files (PDF, PPTX, DOCX) via the Tauri dialog plugin.
2. **Parse**: Rust backend extracts text using `pdf_oxide` (PDF) or `office_oxide` (PPTX/DOCX). Files are associated with the worksheet in SQLite.
3. **Chunk**: Extracted text is split into overlapping chunks of ~512 tokens with 128-token overlap.
4. **Embed**: Each chunk is embedded via `bge-small-en-v1.5` (384-dim) running in `mistralrs`. Embeddings are stored as BLOBs in SQLite alongside their source text.
5. **Store**: Chunks, embeddings, and source references are persisted in SQLite, scoped to the worksheet.
6. **Retrieve**: At generation time, the user's query is embedded with the same model. Cosine similarity is computed in Rust against all worksheet chunks. The top-k most similar chunks form the context.
7. **Generate**: The context is assembled into a few-shot prompt and passed to `SmolLM2-360M-Instruct` via `mistralrs`. The model generates MCQ items, summaries, or mind-map structures depending on the requested artifact type.
8. **Persist**: Generated artifacts are saved to SQLite and returned to the frontend for display in the React workspace.

## Directory Layout

```
worxheet/
├── app/                          # React frontend
│   ├── components/               # UI components
│   │   └── ui/                   # shadcn + base-ui primitives
│   ├── data/                     # TanStack Query hooks + clients
│   ├── lib/                      # Utils, constants
│   ├── routes/                   # TanStack Router routes
│   ├── index.css                 # Tailwind v4 entry
│   └── main.tsx                  # App entry
├── core/                         # Rust backend
│   ├── migrations/               # SQLite migrations
│   ├── src/
│   │   ├── main.rs               # Tauri entry
│   │   ├── lib.rs                # Plugin registration, invoke handler
│   │   ├── database.rs           # SQLite connection + migration runner
│   │   ├── file.rs               # File metadata command
│   │   └── worksheet.rs          # Worksheet CRUD commands
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
All inference runs on-device via `mistralrs` + `candle`. No API keys, no network calls, no third-party model serving. This guarantees privacy and offline operation.

### Embeddings stored as SQLite BLOBs instead of a vector database
At the expected scale (<10,000 chunks per worksheet), brute-force cosine similarity over float32 arrays stored in SQLite BLOBs takes under 1ms. No vector DB (LanceDB, Pinecone, etc.) is needed. This avoids adding ~170MB+ of dependencies and keeps the architecture simple.

### Single inference runtime for embeddings and generation
Both `bge-small-en-v1.5` (embedding encoder) and `SmolLM2-360M-Instruct` (text decoder) run in the same `mistralrs` process. This avoids duplicating the ML runtime and keeps the binary lean. Models are separate GGUF files loaded at runtime.

### Why pure Rust inference (candle) over llama.cpp
`candle` is pure Rust with no C++ build dependencies. It compiles with only `rustc`, avoiding the need for `clang`/`cmake` and the ~5-minute C++ build that `llama-cpp-2` requires. The resulting binary is also smaller since dead-code elimination is more effective with Rust's monomorphization.
