# RAG Pipeline Design

## Overview

This document describes the artifact-generation pipeline as implemented. The full path — upload, parse, segment, store, embed, generate, persist — runs automatically in the background after a worksheet is created: creating a worksheet with files starts a job that exhausts the material end-to-end, and the frontend just waits for it to finish and then shows the artifacts.

Generation is **cloud-hosted**: every worksheet sends one request per `(artifact_type, segment)` pair to an OpenAI-compatible LLM provider (OpenAI, Google Gemini's compatibility endpoint, Ollama, LM Studio, or any custom base URL), fanned out under a configurable concurrency limit. Everything before generation — parsing, segmentation, embedding — stays fully local; only segment texts leave the device at generation time. Deterministic local validation gates every model output before it can be persisted.

## Pipeline Steps

```
Create worksheet ─► Parse ─► Segment ─► Store+Embed ─► Generate (cloud) ─► Validate ─► Persist
                     [local]   [local]     [local ONNX]    [provider API]      [local]
```

### 1. Upload (automatic kick-off)

`create_worksheet` inserts the worksheet, registers each selected file in the `files` table, and calls `pipeline::jobs::start_job`, which spawns the background pipeline. The frontend wiring lives in `create-worksheet-dialog.tsx`; creation is blocked with a pointer to Settings until a provider is configured.

### 2. Parse (`pipeline/ingest.rs`)

Parsers emit typed **blocks** (`Heading(level)` / `Body`) instead of flat text so structure survives downstream:

| Format | Parser | Structure source |
|---|---|---|
| PDF | `pdf_oxide` | span-level extraction + font-size histogram heading detection; plain page text fallback |
| PPTX, DOCX, PPT, DOC | `office_oxide` | markdown export split on `#` headings |

Files are parsed on bounded parallel worker threads (`std::thread::available_parallelism` waves).

### 3. Segment (`pipeline/segment.rs`)

The segmenter turns blocks into contiguous, ordered units — coverage of the material is exhaustive by construction:

1. **Structure-first**: every heading starts a new candidate section; body runs pack up to `TARGET_SEGMENT_TOKENS` (1100). Oversized single paragraphs are pre-split into sentences.
2. **Drift fallback**: any section above `DRIFT_TRIGGER_TOKENS` (450) is checked for internal topic shifts — sentence embeddings are compared across consecutive sentences, smoothed, and cut at confident valleys (`mean − 0.75σ` local minima).
3. **Hard windows**: unbreakable text is token-windowed at `MAX_SEGMENT_TOKENS` (1800).

Undersized neighbors sharing a breadcrumb merge; every input token lands in exactly one segment. There is no chunk overlap and no noise class — both failure modes of the previous HDBSCAN scheme are structurally impossible.

### 4. Store + Embed (`pipeline/ingest.rs`)

Segments insert into the `chunks` table (id, position, heading breadcrumb, text) inside one transaction. Each segment is embedded locally by `embedder.rs` — `nomic-embed-text-v1.5`, int8-quantized ONNX (~137MB, downloaded once), mean-pooled + L2-normalized via ORT — and stored as a float32 BLOB.

### 5. Provider resolution (`pipeline/jobs.rs::resolve_backend`)

Per run, the active `ProviderConfig` (Tauri state) joins its API key from the OS keychain and builds an `OpenAiClient`. Unconfigured or missing-key worksheets fail fast with an actionable message.

### 6. Generate (`pipeline/generate.rs::generate_all`)

Units = `{5 artifact types} × {segments}` (evenly sampled past `MAX_UNITS_PER_TYPE` = 48). All units fan out onto a `tokio::Semaphore` sized by the user's concurrency setting (default 8).

- **Structured outputs**: each request carries `response_format: json_schema` (strict mode); servers that reject it fall back to prompt-only JSON once.
- **Prompt contract** (`system_prompt`): grounding-only facts, no figure/table/media references, no "the passage" meta-references, source-language matching, JSON-only output.
- **Fixed item counts** per type (MCQ 4, essay 3, completion 4 per unit) make yield predictable; MCQ prompts embed one exemplar.
- **Validation loop**: output is parsed and every item passes `pipeline/validate.rs`; failures trigger exactly one retry with fresh seed + corrective feedback listing rejection reasons; still-failing items drop individually.
- **Cross-unit dedup**: near-duplicate questions (word-overlap Jaccard ≥ 0.75) are suppressed across segments.
- **Summary/MindMap**: per-segment section objects merge deterministically into one worksheet-wide artifact (no extra call) — full material coverage, unlike the old sampled-cluster cap.
- Telemetry (request count, approx tokens in/out) streams through the existing progress events.

### 7. Persist (`pipeline/mod.rs::persist_artifacts`)

Validated artifacts insert into the `artifacts` table in one transaction and surface through `get_artifacts`.

## Validation layer (`pipeline/validate.rs`)

Pure functions, no LLM — the deterministic quality floor:

| Check | Defect it kills |
|---|---|
| Option normalization (`A)`/`1.`/`-`/`•` stripping) | inconsistent choice formatting |
| Case-insensitive option uniqueness | duplicated answer choices |
| `answer ∈ options` exact match post-normalization | answer/index mismatches |
| Completion answer must appear in source segment | invented answers |
| Figure/media reference regexes | hallucinated references to invisible figures |
| Lexical grounding ratio vs source segment | off-topic/hallucinated questions |
| Cross-unit question similarity | duplicate questions from overlapping content |

Every check has unit tests built from real observed defects.

## Job runner (`pipeline/jobs.rs`)

`PipelineJobs` (kept in Tauri state as `Arc<PipelineJobs>`) serializes pipelines behind a `tokio::sync::Mutex` gate:

- `start_job` marks the worksheet `running` and spawns the job.
- `run_pipeline`: resolve backend → parse/segment/embed/store → clear stale artifacts → `generate_all` → persist.
- Progress persists and streams as a single `pipeline-progress` event (`phase`, per-type completion counts, request/token counters).
- `resume_stale` re-kicks any worksheet stuck in `running` at startup — including worksheets migrated from the old clustering pipeline.

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

-- Segments live here (historical table name).
CREATE TABLE chunks (
    id TEXT PRIMARY KEY,
    worksheet_id TEXT NOT NULL REFERENCES worksheets(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    heading TEXT,                              -- nearest enclosing heading breadcrumb
    text TEXT NOT NULL,
    embedding BLOB,                            -- 768 × f32 little-endian
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

Migration `20260825000000_segments.sql` drops the `clusters` table, clears derived rows, adds `heading`, removes `cluster_index`, and re-kicks completed worksheets through the new pipeline.

## Connection to Frontend

Exposed Tauri commands:

- `get_file_metadata`
- `get_worksheets`, `get_worksheet`, `create_worksheet`, `delete_worksheet`
- `get_artifacts` — list a worksheet's artifacts for one type, optionally capped at `count` randomly-selected items
- `get_pipeline_status` — current pipeline status
- `get_provider_status`, `set_provider_config`, `validate_provider`, `list_provider_models` — provider settings (keys stored in OS keychain via `keyring`, never returned to the frontend)

Events:

- `pipeline-progress` — `{ worksheet_id, status, phase, artifact_type, done, total, types_done, types_total, requests_done?, tokens_in?, tokens_out?, error }`
- `model-download` — `{ kind: "embedding", done, total }` during the first-run ONNX fetch

Routes: `/settings` (provider setup), `/worksheets/$id` (live progress → tabbed workspace), `/worksheets/$id/mcq|essay|completion` (quizzes over sampled artifacts).

## Module Layout (core/src/)

```
core/src/
├── lib.rs              # register commands, AppState (database + embedder + providers + jobs)
├── main.rs             # Tauri entry
├── database.rs         # SQLite connection + migration runner
├── embedder.rs         # nomic-int8 ONNX embedder + model downloader + EmbedderPool
├── assets/tokenizer.json  # vendored nomic tokenizer (embedded via include_str!)
├── provider/
│   ├── mod.rs          # ArtifactBackend trait, GenerateRequest, ProviderError
│   ├── config.rs       # ProviderConfig presets, keychain storage, ProviderState
│   ├── client.rs       # OpenAI-compatible client (retries, backoff, schema fallback)
│   └── mock.rs         # scripted/routed mock backend for tests
├── commands/           # thin #[tauri::command] wrappers
│   ├── file.rs         # get_file_metadata
│   ├── worksheet.rs    # worksheet CRUD (create_worksheet starts a job)
│   ├── pipeline.rs     # get_artifacts, get_pipeline_status
│   └── provider.rs     # provider settings commands
├── worksheet.rs        # worksheet CRUD + file metadata logic
├── pipeline/
│   ├── mod.rs          # orchestration (process_files, load_segments, persist_artifacts)
│   ├── ingest.rs       # block-aware parsers + parallel parse waves + segment persistence
│   ├── segment.rs      # structure-first + drift-fallback segmenter
│   ├── validate.rs     # deterministic output quality gates
│   ├── generate.rs     # semaphore fan-out generation + assembly + telemetry
│   └── jobs.rs         # PipelineJobs runner (start_job, resume_stale, get_status)
└── schema.rs           # shared structs (Segment, Artifact, ArtifactType, PipelineStatus, …)
```
