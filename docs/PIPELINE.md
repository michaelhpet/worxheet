# RAG Pipeline Design

## Overview

This document describes the artifact-generation pipeline as implemented. The full path — upload, parse, chunk, store, embed, cluster, generate, persist — runs automatically in the background after a worksheet is created: creating a worksheet with files starts a job that exhausts the material end-to-end, and the frontend just waits for it to finish and then shows the artifacts. Generation runs one unit per HDBSCAN topic cluster (question types produce one or more items each; `Summary`/`MindMap` yield a single worksheet-wide artifact).

## Pipeline Steps

```
Create worksheet ─► Parse ─► Chunk ─► Store ─► Embed ─► Cluster ─► Generate ─► Persist
   [automatic — pipeline/jobs.rs background job + SQLite]
```

## Step-by-Step

### 1. Upload (automatic kick-off)

`create_worksheet` inserts the worksheet and registers each selected file in the `files` table (path, name, extension, size). When the worksheet is created with at least one file, the command also calls `pipeline::jobs::start_job`, which spawns the background pipeline for that worksheet. The frontend wiring lives in `create-worksheet-dialog.tsx`.

### 2. Parse

`pipeline/ingest.rs::parse_file` extracts text from each uploaded file:

| Format | Parser |
|---|---|
| PDF | `pdf_oxide` (page-by-page extraction) |
| PPTX, DOCX, PPT, DOC | `office_oxide` |

Output: raw text per file. Files are parsed and chunked on bounded parallel
worker threads (`ingest.rs` dispatches waves sized to
`std::thread::available_parallelism`).

### 3. Chunk

`pipeline/ingest.rs::chunk_text` splits extracted text into overlapping chunks sized by the HF `tokenizers` tokenizer.

```
Config:
  chunk_size: 512 tokens
  overlap: 128 tokens

Example:
  "CHAPTER 1: The Cell\nThe cell is the basic unit of life..."
  → Chunk 0: tokens [0-512)
  → Chunk 1: tokens [384-896)
  → Chunk 2: tokens [768-1280)
  ...
```

### 4. Store

`pipeline/mod.rs::process_files` inserts each chunk into the `chunks` table with its worksheet/file reference and document position, then bumps `worksheets.updated_at`.

```rust
// pipeline/mod.rs — public entry point
pub async fn process_files(
    pool: &SqlitePool,
    worksheet_id: &str,
    file_ids: &[String],
    models: &Arc<ModelPool>,
    on_progress: Option<ProgressFn>,
) -> Result<Vec<Chunk>, String>
```

The `embedding` BLOB column starts `NULL` and is populated by the embed step.

### 5. Embed

`pipeline/embed.rs::embed_missing_chunks` lazily loads the embedding model from `ModelPool` and runs `Embedder` (wraps `bge-small-en-v1.5`, Q8_0 GGUF, 384-dim). Un-embedded chunks are read from the DB, embedded (L2-normalized, so cosine similarity is a plain dot product), and written back to `chunks.embedding` as a float32 BLOB.

```rust
// pipeline/embed.rs — public entry points
pub fn embedding_to_bytes(vec: &[f32]) -> Vec<u8>   // little-endian float32 BLOB
pub fn bytes_to_embedding(bytes: &[u8]) -> Vec<f32> // reverse
```

### 6. Cluster

`pipeline/cluster.rs::rebuild_clusters` runs HDBSCAN over all embedded chunks, persists one centroid per topic cluster into the `clusters` table, and writes each chunk's `cluster_index` (or NULL for noise). `min_cluster_size` scales as `n/200` clamped to `[3,16]`; tiny worksheets fall back to a single cluster.

### 7. Generate

`pipeline/generate.rs::generate_artifacts` wraps `Generator` (`SmolLM2-360M-Instruct`, Q8_0 GGUF, 8K context). It (1) splits the material into one context unit per topic cluster (HDBSCAN noise skipped, clusters ordered by source position), (2) builds a per-unit prompt, and (3) generates schema-constrained JSON. Question-style types persist one artifact per item (1-8 per cluster); `Summary` and `MindMap` merge every unit's section into one worksheet-wide artifact. Each unit derives its seed from the base (`seed + unit_index`) so an exhaustive batch never repeats.

Best-effort: a unit whose output truncates or fails to parse is retried once with a doubled (capped 4096) token budget, then skipped so one bad cluster cannot discard the rest of the batch.

Generation characteristics:

- **Chat template**: prompts are built with the model's built-in chat template (system + user messages, `add_generation_prompt`).
- **Grammar-constrained output**: the artifact JSON schema is compiled to a GBNF grammar (`json_schema_to_grammar`) and sampling runs under `LlamaSampler::grammar`. The model can only emit JSON matching the schema.
- **Sampling chain**: `[grammar?, temp?, top_p, dist(seed)]` applied to the logits via `LlamaTokenDataArray::apply_sampler`. This manual array path is used deliberately to avoid the `llama-cpp-2` grammar-sampler crash on `sampler.sample(ctx, idx)` (utilityai/llama-cpp-rs#1007).
- **Termination**: generation stops on the model's EOG token. When the grammar completes, the grammar sampler masks everything but EOG, so the loop ends cleanly.
- **Parameters**: `temperature` (default 0.7), `top_p` (default 0.9), `max_tokens` (default 1024), `seed` (default 1234). Context window `N_CTX = 8192`, max prompt `6144` tokens.
- **Artifact schemas**: `pipeline/generate.rs` provides one schema per artifact type — `MultipleChoiceQuiz`, `EssayQuiz`, `CompletionQuiz`, `Summary`, `MindMap` — with matching system + task prompt builders. Question-style schemas wrap 1-8 items in an array so one cluster yields multiple artifacts; `Summary`/`MindMap` keep single-object schemas whose per-cluster outputs the pipeline concatenates.

### 8. Persist

Validated artifacts are inserted into the `artifacts` table (with `artifact_type` and `source`) and listed back to the frontend by the `get_artifacts` command.

## Job runner (pipeline/jobs.rs)

`PipelineJobs` (kept in Tauri state as `Arc<PipelineJobs>`) runs one pipeline at a time:

- A `tokio::sync::Mutex` gate serializes pipelines — only one llama.cpp inference job runs at once, regardless of how many worksheets are created quickly.
- `start_job` marks the worksheet `running` in the DB and spawns the job on the Tauri async runtime.
- `run_pipeline` → `process_files` (ingest + embed + cluster), clears the worksheet's existing `artifacts` rows, then generates each of the five artifact types in order.
- Progress is persisted and streamed as a single `pipeline-progress` event; on completion the worksheet's `pipeline_status` is set to `done` (or `failed` with an error message in `pipeline_error`).
- `resume_stale` runs at app startup and re-kicks any worksheet stuck in `running` (e.g. after a crash), so a job is never lost silently.

## Data Model (SQLite)

```sql
CREATE TABLE worksheets (
    id VARCHAR(255) NOT NULL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    pipeline_status VARCHAR(255) NOT NULL DEFAULT 'idle',   -- idle | running | done | failed
    pipeline_error TEXT                                      -- set when status = 'failed'
);

CREATE TABLE files (
    id TEXT PRIMARY KEY,
    worksheet_id TEXT NOT NULL REFERENCES worksheets(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    name TEXT NOT NULL,
    extension TEXT NOT NULL,
    size INTEGER NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE chunks (
    id TEXT PRIMARY KEY,
    worksheet_id TEXT NOT NULL REFERENCES worksheets(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    text TEXT NOT NULL,
    embedding BLOB,                            -- 1536 bytes (384 × f32), NULL until embed step runs
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE artifacts (
    id TEXT PRIMARY KEY,
    worksheet_id TEXT NOT NULL REFERENCES worksheets(id) ON DELETE CASCADE,
    artifact_type TEXT NOT NULL,               -- 'MultipleChoiceQuiz' | 'Summary' | ...
    source TEXT NOT NULL,
    content TEXT NOT NULL,                     -- JSON payload
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```

Pipeline status columns were added by `migrations/20260816000000_pipeline_status.sql`.

## Connection to Frontend

Exposed Tauri commands (`lib.rs` invoke handler):

- `get_file_metadata`
- `get_worksheets`, `get_worksheet`, `create_worksheet`, `delete_worksheet`
- `get_artifacts` — list a worksheet's artifacts for one type, optionally
  capped at `count` randomly-selected items (used by the quiz pages to sample
  questions)
- `get_pipeline_status` — read the worksheet's current pipeline status

`create_worksheet` starts the background job when files are provided; `delete_worksheet` stops/removes any in-flight job.

Progress event (listened via `@tauri-apps/api/event`):

- `pipeline-progress` — `{ worksheet_id, status, phase, artifact_type, done, total, types_done, types_total, error }` emitted throughout the run
- `model-download` — `{ kind, done, total }` while the first run downloads GGUFs

The React worksheet detail route (`app/routes/worksheets.$id.tsx`) shows live
progress while the pipeline runs (`usePipelineStatus` in `app/data/pipeline.ts`,
polling `get_pipeline_status`). Once done it becomes a tabbed workspace: each
quiz type opens a setup card (question count + optional timer) that launches a
dedicated quiz route — `/worksheets/$id/mcq|essay|completion` — over artifacts
sampled via `useArtifacts(id, type, count)`; see ARCHITECTURE.md's
[Quiz Flow](ARCHITECTURE.md#quiz-flow) for the quiz architecture. There are no
process/generate buttons — creating a worksheet is the entire generation
interaction.

## Module Layout (core/src/)

```
core/src/
├── lib.rs              # register commands, manage AppState (database + models + jobs), resume_stale
├── main.rs             # Tauri entry
├── database.rs         # SQLite connection + migration runner
├── models.rs           # GGUF/tokenizer paths + hf-hub download + lazy ModelPool + Embedder + Generator
├── commands/           # thin #[tauri::command] wrappers (State → domain logic)
│   ├── file.rs         # get_file_metadata command
│   ├── worksheet.rs    # worksheet CRUD commands (create_worksheet starts a job)
│   └── pipeline.rs     # get_artifacts, get_pipeline_status commands
├── worksheet.rs        # worksheet CRUD + file metadata logic
├── pipeline/           # background pipeline (testable cores)
│   ├── mod.rs          # orchestration (process_files) + integration tests
│   ├── ingest.rs       # parse_file + chunk_text + ingest loop (parallel parse waves)
│   ├── embed.rs        # bge-small embeddings + BLOB encode/decode
│   ├── cluster.rs      # HDBSCAN clustering + cluster_contexts + rebuild_clusters
│   ├── generate.rs     # SmolLM2 grammar-constrained generation + assemble_artifacts
│   └── jobs.rs         # PipelineJobs runner (start_job, resume_stale, get_status)
└── schema.rs           # shared structs (Chunk, Artifact, ArtifactType, PipelineStatus, …)
```