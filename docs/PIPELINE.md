# RAG Pipeline Design

## Overview

This document describes the artifact-generation pipeline as implemented. The full path — upload, parse, chunk, store, embed, retrieve, generate, persist — is wired end-to-end through Tauri commands (`pipeline.rs`) and SQLite. Clustering / topic-sampling for multi-artifact generation is planned but not built; for now `generate_artifacts` uses the top-k chunks most similar to the artifact task as context.

## Pipeline Steps

```
Upload ─► Parse ─► Chunk ─► Store ─► Embed ─► Retrieve ─► Generate ─► Persist
   [wired — pipeline.rs commands + SQLite]
```

## Step-by-Step

### 1. Upload

`create_worksheet` inserts the worksheet and registers each selected file in the `files` table (path, name, extension, size). The wiring between the file dialog and the worksheet lives in the frontend (`create-worksheet-dialog.tsx`).

### 2. Parse

`ingest.rs::parse_file` extracts text from each uploaded file:

| Format | Parser |
|---|---|
| PDF | `pdf_oxide` (page-by-page extraction) |
| PPTX, DOCX, PPT, DOC | `office_oxide` |

Output: raw text per file. File status is updated to `parsing` before extraction and `parsed` on success.

### 3. Chunk

`chunk.rs::chunk_text` splits extracted text into overlapping chunks sized by the HF `tokenizers` tokenizer.

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

`pipeline.rs::run_process_files` inserts each chunk into the `chunks` table with its worksheet/file reference and document position, then bumps `worksheets.updated_at`. The same command drives file parsing end-to-end; it is exposed to the frontend as the `process_files` command.

```rust
// pipeline.rs — public entry point
pub async fn run_process_files(
    pool: &SqlitePool,
    worksheet_id: &str,
    file_ids: &[String],
    models: &Arc<ModelPool>,
) -> Result<Vec<Chunk>, String>
```

The `embedding` BLOB column starts `NULL` and is populated by the embed step.

### 5. Embed (wired)

`pipeline.rs::run_embed_worksheet` lazily loads the embedding model from `ModelPool` and runs `embed.rs::Embedder` (wraps `bge-small-en-v1.5`, Q8_0 GGUF, 384-dim). Un-embedded chunks are read from the DB, embedded (L2-normalized, so cosine similarity is a plain dot product), and written back to `chunks.embedding` as a float32 BLOB.

```rust
// embed.rs — public entry point
pub struct Embedder { /* ... */ }

pub fn load(model_path: &Path) -> Result<Self, String>
pub fn dimension(&self) -> usize          // 384
pub fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String>
```

Status: wired. `embed_worksheet` persists vectors to the BLOB column; `generate_artifacts` also auto-embeds any chunks that are still missing vectors, so a worksheet can skip the explicit embed step.

### 6. Retrieve (wired)

`pipeline.rs::run_retrieve_chunks` embeds a query and returns the top-k chunks by cosine similarity (`retrieval.rs`). BLOBs are decoded to `Vec<f32>` and compared by dot product on the L2-normalized vectors.

```rust
// retrieval.rs — public entry point
pub async fn retrieve(
    pool: &SqlitePool,
    worksheet_id: &str,
    query: &str,
    k: usize,
) -> Result<Vec<RetrievedChunk>, String>
```

### 7. Generate (wired)

`pipeline.rs::run_generate_artifacts` wraps `generation.rs::Generator` (`SmolLM2-360M-Instruct`, Q8_0 GGUF, 8K context). It (1) auto-embeds any un-embedded chunks, (2) retrieves the top-k chunks most similar to the artifact task, (3) builds the task prompt with that context, and (4) generates schema-constrained JSON.

```rust
// generation.rs — public entry points
pub fn load(model_path: &Path) -> Result<Self, String>
pub fn apply_chat_template(&self, system: &str, user: &str) -> Result<String, String>
pub fn generate(
    &self,
    prompt: &str,
    schema_json: Option<&str>,
    params: &GenerationParams,
) -> Result<String, String>
```

Generation characteristics:

- **Chat template**: prompts are built with the model's built-in chat template (system + user messages, `add_generation_prompt`).
- **Grammar-constrained output**: the artifact JSON schema is compiled to a GBNF grammar (`json_schema_to_grammar`) and sampling runs under `LlamaSampler::grammar`. The model can only emit JSON matching the schema.
- **Sampling chain**: `[grammar?, temp?, top_p, dist(seed)]` applied to the logits via `LlamaTokenDataArray::apply_sampler`. This manual array path is used deliberately to avoid the `llama-cpp-2` grammar-sampler crash on `sampler.sample(ctx, idx)` (utilityai/llama-cpp-rs#1007).
- **Termination**: generation stops on the model's EOG token. When the grammar completes, the grammar sampler masks everything but EOG, so the loop ends cleanly.
- **Parameters**: `temperature` (default 0.7), `top_p` (default 0.9), `max_tokens` (default 1024), `seed` (default 1234). Context window `N_CTX = 8192`, max prompt `6144` tokens.
- **Artifact schemas**: `schema.rs` provides one schema per artifact type — `MultipleChoiceQuiz`, `EssayQuiz`, `CompletionQuiz`, `Summary`, `MindMap` — with matching system + task prompt builders.

Status: wired. `generate_artifacts` writes the validated artifact JSON to the `artifacts` table and returns it to the frontend. It emits `generation-progress` events via the `AppHandle` while sampling.

### 8. Persist (wired)

Validated artifacts are inserted into the `artifacts` table (with `artifact_type` and `source`) and listed back to the frontend by the `get_artifacts` command.

## Data Model (SQLite)

```sql
CREATE TABLE files (
    id TEXT PRIMARY KEY,
    worksheet_id TEXT NOT NULL REFERENCES worksheets(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    name TEXT NOT NULL,
    extension TEXT NOT NULL,
    size INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'uploaded',   -- uploaded | parsing | parsed
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

## Connection to Frontend

Exposed Tauri commands (`lib.rs` invoke handler):

- `get_file_metadata`
- `get_worksheets`, `get_worksheet`, `create_worksheet`, `delete_worksheet`
- `get_files` — list a worksheet's files with parse status
- `process_files` — parse + chunk + store selected files
- `embed_worksheet` — embed all un-embedded chunks and persist BLOBs
- `retrieve_chunks` — RAG retrieval (top-k chunks by cosine similarity)
- `generate_artifacts` — auto-embed, retrieve context, generate + persist an artifact (takes `worksheet_id`, `artifact_type`, optional `topic`, `GenerationParams`)
- `get_artifacts` — list a worksheet's generated artifacts

Pending (next steps):

- React workspace view in the worksheet detail route (listing artifacts, generation controls, progress indicators for the `generation-progress` events)

## Module Layout (core/src/)

```
core/src/
├── lib.rs              # register commands, manage AppState (database + models)
├── main.rs             # Tauri entry
├── database.rs         # SQLite connection + migration runner
├── llm.rs              # Process-wide llama.cpp backend singleton
├── models.rs           # GGUF/tokenizer paths + hf-hub download + lazy ModelPool
├── file.rs             # get_file_metadata command
├── worksheet.rs        # worksheet CRUD commands
├── chunk.rs            # text splitting logic
├── ingest.rs           # parse + chunk + store pipeline
├── embed.rs            # bge-small embeddings
├── retrieval.rs        # embedding BLOB encode/decode + cosine retrieval
├── generation.rs       # SmolLM2 grammar-constrained generation
├── pipeline.rs         # end-to-end commands + testable run_* cores
└── schema.rs           # shared structs (Chunk, Artifact, ArtifactType, GenerationParams, …)
```
