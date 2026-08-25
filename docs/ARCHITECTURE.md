# Architecture

## Overview

Worxheet is a desktop monolith with a **local-first ingest, hosted generation** split: parsing, segmentation, and embedding run entirely on the user's machine; artifact generation calls an OpenAI-compatible LLM provider (OpenAI, Gemini, Ollama, LM Studio, or any custom endpoint) chosen in Settings. No GGUF LLM ships with or downloads into the app — the only model artifact is a ~137MB quantized ONNX embedder fetched on first use.

## System Diagram

```
┌─────────────────────────────────────────────────────┐
│                   Tauri Process                       │
│  ┌─────────────────────┐  ┌───────────────────────┐  │
│  │   Frontend (React)   │  │   Backend (Rust)       │  │
│  │   TanStack Router    │◄─┤   IPC Commands          │  │
│  │   TanStack Query     │──►│   provider API (LLM)    │  │
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

```
Upload ─► Parse ─► Segment ─► Store ─► Embed ─► Generate (provider API) ─► Validate ─► Persist
          [local]  [local]     [local]   [local]        [cloud/local-server]      [local]
```

All stages run automatically in a background job when a worksheet is created (`pipeline/jobs.rs`).
Segments are contiguous, ordered units produced by `pipeline/segment.rs`; every segment feeds one
request per artifact type, fanned out under a concurrency semaphore.

1. **Parse**: block-aware extraction — PDFs via span/font-size heading detection (`pdf_oxide`),
   Office formats via markdown headings (`office_oxide`).
2. **Segment**: structure-first packing, embedding-drift fallback for topic shifts inside
   unstructured stretches, hard token windows as last resort. Exhaustive by construction.
3. **Store + Embed**: segments persist to SQLite; vectors come from a local
   `nomic-embed-text-v1.5` int8 ONNX model through ONNX Runtime.
4. **Generate**: `(5 types × segments)` requests against the configured provider under
   `response_format: json_schema` strict mode, validated locally with one corrective retry.
5. **Persist**: surviving items insert as artifacts; Summary/MindMap merge deterministically
   into worksheet-wide artifacts covering every segment.

## Quiz Flow

Generated quiz artifacts double as takeable quizzes. Each of the three quiz
types has a dedicated route — `/worksheets/$id/mcq`,
`/worksheets/$id/completion`, `/worksheets/$id/essay` — sharing one custom
wizard component (`app/components/quiz-shell.tsx`). The questionnaire
primitive from `@shadcn/react` was evaluated and rejected: it gates
Next/Submit on optional items, which conflicts with the "no answer is
required, blanks fail silently" rule.

- **Setup**: each quiz tab on the worksheet detail page renders a setup card
  (`quiz-tab-content.tsx`) — a number-of-questions slider bounded by the
  available artifacts (`worksheet.artifact_counts`) and an optional timer
  built from multiplier presets per quiz type. Starting navigates to the
  route with `{count, time}` search params (zod-validated).
- **Sampling**: quiz routes fetch their questions via
  `useArtifacts(id, type, count)` → `get_artifacts`, which caps the pool at
  `count` randomly-selected items of that artifact type.
- **Wizard**: `QuizShell` shows one question at a time with
  Previous/Next/Submit. All questions stay mounted inside the form
  (non-active ones toggled with the `hidden` attribute), so uncontrolled
  inputs survive step changes and FormData captures every answer. Nothing is
  required — blanks simply grade as failed.
- **Timer**: when `time` (minutes) is set, a HH:MM:SS countdown header
  (`input-otp` slots) ticks down with urgency styling under 60s remaining;
  expiry auto-submits via `useCountdown`.
- **Dialogs**: submitting and leaving both require confirmation (AlertDialog).
- **Grading**: on submit the FormData is compared against the answers stored
  in the artifacts — exact match against the option string for MCQ, trimmed
  case-insensitive match for completion sentences; essays carry no auto
  grade (`correct: null`) and are excluded from the score.
- **Results**: `QuizGrade` (`app/components/quiz-grade.tsx`) swaps in place:
  tiered verdict (Excellent ≥80% / Good ≥50% / otherwise), percentage
  headline plus raw correct count, and a per-question breakdown (✓ / ✗ /
  "review", your answer vs the correct or model answer). "Back to worksheet"
  returns to the detail page.
- **MCQ choices** use the shadcn RadioGroup (`radio-group.tsx`): base-ui's
  radio items render real hidden `<input type="radio">` elements, so the
  FormData-based grading works without any bridging state.

## Inference

- **Embedding (local)**: `nomic-embed-text-v1.5` int8-quantized ONNX (~137MB) runs through
  `ort` on CPU (`embedder.rs`). The tokenizer is vendored into the binary; the ONNX artifact
  downloads once from a pinned mirror into the app data directory. Vectors are mean-pooled,
  L2-normalized, 768-dim.
- **Generation (hosted)**: an OpenAI-compatible `/chat/completions` client (`provider/client.rs`)
  talks to the configured provider. Requests carry a strict JSON schema; 429/5xx retry with
  exponential backoff honoring `Retry-After`; servers rejecting `response_format` fall back to
  prompt-only JSON once. API keys live in the OS keychain (`keyring`) and never reach the
  frontend.

## Directory Layout

```
worxheet/
├── app/                          # React frontend
│   ├── components/               # UI components (layout, create-worksheet-dialog, files-uploader,
│   │                             #   file-card, quiz-shell, quiz-grade, quiz-tab-content, theme-provider, ui/…)
│   ├── data/                     # TanStack Query hooks + IPC wrappers (worksheets, artifacts, pipeline, model-downloads)
│   ├── lib/                      # Types, utils, constants, use-countdown
│   ├── routes/                   # File-based TanStack Router routes (index, worksheet detail, mcq/completion/essay quiz pages)
│   ├── index.css                 # Tailwind v4 entry
│   └── main.tsx                  # App entry
├── core/                         # Rust backend
│   ├── migrations/               # SQLite migrations
│   ├── src/
│   │   ├── main.rs               # Tauri entry
│   │   ├── lib.rs                # Plugin registration, AppState (database + embedder + providers + jobs), invoke handler, resume_stale
│   │   ├── database.rs           # SQLite connection + migration runner
│   │   ├── embedder.rs           # nomic-int8 ONNX embedder (ort) + model downloader + EmbedderPool
│   │   ├── commands/             # Thin #[tauri::command] wrappers (State → domain logic)
│   │   │   ├── file.rs           # get_file_metadata command
│   │   │   ├── worksheet.rs      # Worksheet CRUD commands (create_worksheet starts a job)
│   │   │   └── pipeline.rs       # get_artifacts, get_pipeline_status commands
│   │   ├── worksheet.rs          # Worksheet CRUD + file metadata logic
│   │   ├── pipeline/             # Background pipeline (testable cores)
│   │   │   ├── mod.rs            # Orchestration (process_files) + integration tests
│   │   │   ├── ingest.rs         # parse_file + chunk_text + ingest loop
│   │   │   ├── embed.rs          # bge-small embedding + BLOB encode/decode
│   │   │   ├── cluster.rs        # HDBSCAN clustering + cluster_contexts + rebuild_clusters
│   │   │   ├── generate.rs       # SmolLM2 grammar-constrained generation + assemble_artifacts
│   │   │   └── jobs.rs           # PipelineJobs runner (start_job, resume_stale, get_status)
│   │   └── schema.rs             # Shared structs (Chunk, Artifact, ArtifactType, PipelineStatus, …)
│   ├── Cargo.toml
│   └── tauri.conf.json
├── docs/                         # Project documentation
├── index.html
├── package.json
├── tsconfig.json
└── vite.config.ts
```

## Key Design Decisions

### Local-first ingest, hosted generation
Parsing, segmentation, and embedding stay fully offline. Generation requires a user-configured
LLM provider: cloud keys for hosted models, or a local Ollama/LM Studio server for users who
want zero third parties. Segment texts are the only material that leaves the device, and only at
generation time. This trade was made deliberately: small local models produced unreliable,
hallucination-prone assessments even under grammar constraints.

### Segmentation instead of clustering
The original pipeline clustered chunk embeddings with HDBSCAN and sampled ten chunks per cluster
into each prompt — a workaround for a 6K-token local context window. Hosted contexts removed that
constraint, and clustering's costs remained (noise loss, scattered patchwork prompts, fragile
hyperparameters). The segmenter produces contiguous ordered units instead; HDBSCAN, linfa, and
ndarray left the dependency tree entirely.

### Embeddings stored as SQLite BLOBs instead of a vector database
At the expected scale (<10,000 chunks per worksheet), brute-force clustering and context selection over float32 arrays stored in SQLite BLOBs takes under 1ms. No vector DB (LanceDB, Pinecone, etc.) is needed. This avoids adding ~170MB+ of dependencies and keeps the architecture simple.

### ONNX Runtime for local inference
The embedder runs through `ort` with a quantized nomic model — no C++ toolchain needed to build,
no GGUF downloads. The `ArtifactBackend` trait keeps generation pluggable; today's only
implementation is OpenAI-compatible HTTP.

