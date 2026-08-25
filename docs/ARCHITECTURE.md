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
Upload ─► Parse ─► Chunk ─► Store ─► Embed ─► Cluster ─► Generate ─► Persist
```
All stages run automatically in a background job when a worksheet is created
(`pipeline/jobs.rs`). `process_files` embeds and runs HDBSCAN clustering after
chunking, and `generate_artifacts` exhausts the material one unit per topic
cluster. There is no per-file retrieval step: chunk vectors are stored as BLOBs
and used directly for clustering and context selection.

1. **Upload** *(done)*: The user creates a worksheet and selects files (PDF, PPTX, DOCX) via the Tauri dialog plugin. `create_worksheet` registers the files in SQLite and starts the background pipeline job.
2. **Parse** *(done)*: `pipeline/ingest.rs` extracts text using `pdf_oxide` (PDF) or `office_oxide` (PPTX/DOCX/PPT/DOC).
3. **Chunk** *(done)*: Extracted text is split into overlapping chunks of ~512 tokens with 128-token overlap via `chunk_text` (HF `tokenizers`).
4. **Store** *(done)*: `pipeline/mod.rs::process_files` inserts chunks into the `chunks` table (text + position + file/worksheet reference).
5. **Embed** *(done)*: `pipeline/embed.rs::embed_missing_chunks` embeds chunks without an embedding via `bge-small-en-v1.5` (Q8_0, 384-dim, L2-normalized) and stores the vectors in the `chunks.embedding` BLOB.
6. **Cluster** *(done)*: `pipeline/cluster.rs::rebuild_clusters` runs HDBSCAN over the embedded chunks and persists cluster centroids + per-chunk `cluster_index`.
7. **Generate** *(done)*: `pipeline/generate.rs::generate_artifacts` splits the material into one prompt per topic cluster (ordered by source position) and generates schema-constrained JSON via a GBNF grammar (`json_schema_to_grammar`). Question types (`MultipleChoiceQuiz`, `EssayQuiz`, `CompletionQuiz`) emit 1-8 items per cluster, each persisted as its own artifact; `Summary` and `MindMap` merge per-cluster sections into a single worksheet-wide artifact. Per-unit seeds (`seed + index`) keep the batch from repeating itself.
8. **Persist** *(done)*: Generated artifacts are validated, inserted into the `artifacts` table, and returned to the frontend via `get_artifacts`.

The React worksheet detail route (`app/routes/worksheets.$id.tsx`) is the
user-facing workspace: while the pipeline runs it shows live progress
(`usePipelineStatus`, polling `get_pipeline_status`); once done it becomes a
tabbed view over the five artifact types. The three quiz types open a quiz
setup card, and "Start quiz" navigates to that type's quiz route (see
[Quiz Flow](#quiz-flow) below). Summary/MindMap tabs are placeholders pending
visualization work. Creating a worksheet is still the whole generation
interaction — there are no process/generate buttons.

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

Both models run in the single llama.cpp backend created once per process.

- `models.rs` holds a process-wide `LlamaBackend` in a `OnceLock`. `llama_backend_init` may only run once, so models and contexts are created from a `&'static LlamaBackend`.
- `models.rs` also exposes a `ModelPool` (kept in Tauri state as `Arc<ModelPool>`). It lazily downloads and loads three artifacts on first use: the embedding GGUF, the generation GGUF, and the generation model's `tokenizer.json` (used for chunking).
- Embedding model: `bge-small-en-v1.5` (Q8_0 GGUF, 384-dim).
- Generation model: `SmolLM2-360M-Instruct` (Q8_0 GGUF, 8K context).
- All three artifacts are downloaded from Hugging Face on first use via `hf-hub` and stored in the app data directory (`models_dir`); tests override the location with `WORXHEET_MODELS_DIR`.
- Generation samples under a GBNF grammar chain `[grammar, temp?, top_p, dist(seed)]` using the apply-sampler path (`LlamaTokenDataArray::from_iter(ctx.get_logits_ith(idx))` + `apply_sampler` + `selected_token`) rather than `sampler.sample(ctx, idx)`, which crashes with grammar chains in llama-cpp-2 (see the "Grammar-constrained generation" decision below).

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
│   │   ├── lib.rs                # Plugin registration, AppState (database + models + jobs), invoke handler, resume_stale
│   │   ├── database.rs           # SQLite connection + migration runner
│   │   ├── models.rs             # GGUF model paths + hf-hub download + lazy ModelPool + Embedder + Generator
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

### Local-first, zero cloud dependencies
All inference runs on-device via `llama-cpp-2`. No API keys, no third-party model serving. This guarantees privacy and offline operation. Models are downloaded once, lazily on first use, and stored in the app data directory.

### Embeddings stored as SQLite BLOBs instead of a vector database
At the expected scale (<10,000 chunks per worksheet), brute-force clustering and context selection over float32 arrays stored in SQLite BLOBs takes under 1ms. No vector DB (LanceDB, Pinecone, etc.) is needed. This avoids adding ~170MB+ of dependencies and keeps the architecture simple.

### Single llama.cpp backend for embeddings and generation
Both `bge-small-en-v1.5` (embedding encoder) and `SmolLM2-360M-Instruct` (text decoder) run through the same llama.cpp backend, initialized once per process (in `models.rs`). Models are separate Q8_0 GGUF files loaded at runtime.

### Grammar-constrained generation
Artifacts are generated as structured JSON by compiling a JSON schema into a GBNF grammar (`json_schema_to_grammar`) and sampling under that grammar with `LlamaSampler::grammar`. This guarantees schema-valid output from the small model without post-hoc parsing fixes.

Sampling uses the apply-sampler API (`LlamaTokenDataArray::from_iter(ctx.get_logits_ith(idx))` → `apply_sampler` → `selected_token`) instead of `sampler.sample(ctx, idx)`, because the single-call `sample` path triggers a `GGML_ASSERT(!stacks.empty())` crash in llama-cpp-2 0.1.145+ (upstream issue `utilityai/llama-cpp-rs#1007`). Generation terminates when the empty grammar stack masks every non-EOG token, forcing an EOG token to be selected.

### Inference via llama-cpp-2 (switched from candle/mistralrs)
The project uses `llama-cpp-2` 0.1.154. This is a change from the original candle-based `mistralrs` plan; the bundled C++ build of llama.cpp requires `cmake`/`clang` at build time (~5 minute compile) in exchange for a battle-tested inference engine and the GBNF grammar sampler.
