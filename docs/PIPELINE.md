# RAG Pipeline Design

## Overview

This document describes the artifact-generation pipeline as currently implemented. The wired end-to-end path today covers uploading, parsing, chunking, and storing chunks in SQLite (`ingest.rs`). Embedding and generation are implemented as tested modules but are not yet exposed as Tauri commands or persisted to the database. Clustering / topic-sampling for multi-artifact generation is planned but not built.

## Pipeline Steps

```
Upload ─► Parse ─► Chunk ─► Store        [wired — ingest.rs]
   Embed                                [module implemented + tested]
   Generate (grammar-constrained JSON)  [module implemented + tested]
   RAG retrieve / cluster / persist     [planned]
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

`ingest.rs::process_files` inserts each chunk into the `chunks` table with its worksheet/file reference and document position, then bumps `worksheets.updated_at`.

```rust
// ingest.rs — public entry point
pub async fn process_files(
    pool: &sqlx::SqlitePool,
    worksheet_id: &str,
    file_ids: &[String],
    tokenizer: &Tokenizer,
) -> Result<Vec<Chunk>, String>
```

The `embedding` BLOB column is present but currently left `NULL` (see Embed below).

### 5. Embed (module implemented)

`embed.rs::Embedder` wraps `bge-small-en-v1.5` (Q8_0 GGUF, 384-dim) via llama.cpp. Texts are embedded one per decode call and L2-normalized (unit norm), so cosine similarity is a plain dot product.

```rust
// embed.rs — public entry point
pub struct Embedder { /* ... */ }

pub fn load(model_path: &Path) -> Result<Self, String>
pub fn dimension(&self) -> usize          // 384
pub fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String>
```

Status: the module builds and passes unit tests (`test_embed_similarity`). Storing vectors into the `chunks.embedding` BLOB and exposing an embed command are pending.

### 6. Generate (module implemented)

`generation.rs::Generator` wraps `SmolLM2-360M-Instruct` (Q8_0 GGUF, 8K context) via llama.cpp.

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
- **Grammar-constrained output**: when `schema_json` is provided, the schema is compiled to a GBNF grammar (`json_schema_to_grammar`) and sampling runs under `LlamaSampler::grammar`. The model can only emit JSON matching the schema.
- **Sampling chain**: `[grammar?, temp?, top_p, dist(seed)]` applied to the logits via `LlamaTokenDataArray::apply_sampler`. This manual array path is used deliberately to avoid the `llama-cpp-2` grammar-sampler crash on `sampler.sample(ctx, idx)` (utilityai/llama-cpp-rs#1007).
- **Termination**: generation stops on the model's EOG token. When the grammar completes, the grammar sampler masks everything but EOG, so the loop ends cleanly.
- **Parameters**: `temperature` (default 0.7), `top_p` (default 0.9), `max_tokens` (default 1024), `seed` (default 1234). Context window `N_CTX = 8192`, max prompt `6144` tokens.
- **Artifact schema**: e.g. an MCQ object with `question` and 4 `options`; the caller supplies the JSON schema.

Status: the module builds and passes `test_generate_grammar_json` (validates the output parses as JSON matching the schema). It is not yet exposed as a Tauri command, does not yet receive RAG context, and does not yet write to `artifacts`.

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
    embedding BLOB,                            -- 1536 bytes (384 × f32), currently NULL
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

Currently exposed Tauri commands (`lib.rs` invoke handler):

- `get_file_metadata`
- `get_worksheets`, `get_worksheet`, `create_worksheet`, `delete_worksheet`

Pending wiring (next steps):

- `process_files` (ingest → chunks) as a command
- Embed chunks and persist to `chunks.embedding`
- RAG retrieval (embed the prompt, cosine similarity over chunk BLOBs, top-k context)
- `generate_artifacts` → persist to `artifacts` → return to the frontend
- Progress events during generation and a workspace view in the worksheet detail route

## Module Layout (core/src/)

```
core/src/
├── lib.rs              # register commands, manage AppState
├── main.rs             # Tauri entry
├── database.rs         # SQLite connection + migration runner
├── llm.rs              # Process-wide llama.cpp backend singleton
├── models.rs           # GGUF model paths + hf-hub download
├── file.rs             # get_file_metadata command
├── worksheet.rs        # worksheet CRUD commands
├── chunk.rs            # text splitting logic
├── ingest.rs           # parse + chunk + store pipeline
├── embed.rs            # bge-small embeddings (module)
├── generation.rs       # SmolLM2 grammar-constrained generation (module)
└── schema.rs           # shared structs (Chunk, Cluster, Artifact, etc.)
```
