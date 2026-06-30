# RAG Pipeline Design

## Overview

Artifact generation is automatic — the user uploads materials and clicks "Generate". There is no user query. The pipeline ingests all documents, chunks them, embeds them, clusters by topic, and generates artifacts from cluster centroids.

## Pipeline Steps

```
Upload → Parse → Chunk → Embed → k-means cluster → Pick centroids → SmolLM2 generate → Persist
```

## Step-by-Step

### 1. Parse

Extract text from uploaded files using the appropriate parser:

| Format | Parser |
|---|---|
| PDF | `pdf_oxide` |
| PPTX, DOCX, PPT, DOC | `office_oxide` |

Output: raw text per file.

### 2. Chunk

Split extracted text into overlapping chunks.

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

Each chunk is stored in SQLite with a reference to its source file and position.

### 3. Embed

Each chunk is passed through `bge-small-en-v1.5` (via `mistralrs`) to produce a 384-dimensional float32 vector.

```
Chunk text → bge-small → [0.12, -0.45, 0.78, ..., 0.03] (384 f32s)
```

Embeddings are stored as BLOBs in SQLite alongside their chunk, within the same transaction.

### 4. Cluster (k-means)

When generation is triggered, all embeddings for the worksheet are loaded and clustered.

```rust
fn k_means(chunks: &[Chunk], k: usize, max_iters: usize) -> Vec<Cluster>

fn choose_k(num_chunks: usize) -> usize {
    (num_chunks / 20).clamp(5, 30)
}
```

- Implementation: standard Lloyd's algorithm
- Initialization: k-means++ (spreads initial centroids to avoid empty clusters)
- Distance: cosine similarity
- Convergence: break when no assignments change, or after `max_iters` (default 50)
- Expected runtime: <50ms for ~5000 384-dim vectors

Output: `Vec<Cluster>` where each `Cluster` has a centroid vector and a list of member chunk IDs.

### 5. Sample Centroids

For each cluster, pick the chunk whose embedding is nearest to the cluster centroid (the "most representative" chunk of that topic).

```rust
fn centroid_chunk(cluster: &Cluster, chunks: &[Chunk]) -> &Chunk {
    cluster.member_ids
        .iter()
        .map(|id| chunks.iter().find(|c| c.id == *id).unwrap())
        .min_by(|a, b| cosine_sim(a.embedding, cluster.centroid)
            .partial_cmp(&cosine_sim(b.embedding, cluster.centroid))
            .unwrap())
        .unwrap()
}
```

The centroid chunk's text becomes the context for generation.

### 6. Generate (SmolLM2-360M)

One LLM call per cluster. Each call sends the centroid chunk text wrapped in an artifact-specific prompt.

```
Artifact types:
  - Quiz: "Generate 3 MCQ questions testing higher-order thinking..."
  - Summary: "Write a focused summary..."
  - MindMap: "Extract key concepts and their relationships..."

System prompt structure per call:
  "You are an educational assessment generator. Based on the following
   textbook passage, generate [artifact_type]. Return only valid JSON.
   
   Passage:
   [centroid_chunk_text]"
```

Each generation result is a structured artifact (e.g., JSON for MCQ items with question, options, correct answer, explanation).

### 7. Persist

Generated artifacts are saved to SQLite with references to the worksheet and the source chunk cluster.

## Data Model (SQLite additions to `worksheets`)

```sql
-- Already exists
CREATE TABLE worksheets (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- New tables needed
CREATE TABLE files (
    id TEXT PRIMARY KEY,
    worksheet_id TEXT NOT NULL REFERENCES worksheets(id),
    path TEXT NOT NULL,
    name TEXT NOT NULL,
    extension TEXT NOT NULL,
    size INTEGER NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE chunks (
    id TEXT PRIMARY KEY,
    worksheet_id TEXT NOT NULL REFERENCES worksheets(id),
    file_id TEXT NOT NULL REFERENCES files(id),
    position INTEGER NOT NULL,
    text TEXT NOT NULL,
    embedding BLOB,  -- 1536 bytes (384 × f32)
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE artifacts (
    id TEXT PRIMARY KEY,
    worksheet_id TEXT NOT NULL REFERENCES worksheets(id),
    artifact_type TEXT NOT NULL,  -- 'quiz' | 'summary' | 'mind_map'
    source_chunk_id TEXT REFERENCES chunks(id),
    content TEXT NOT NULL,         -- JSON payload
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```

## Connection to Frontend

When the user clicks "Generate", the frontend calls:

```rust
#[tauri::command]
async fn generate_artifacts(
    state: State<'_, AppState>,
    worksheet_id: String,
    artifact_type: ArtifactType,  // Quiz | Summary | MindMap
) -> Result<Vec<Artifact>, String>
```

The command streams progress via Tauri events:

```rust
app_handle.emit("generation-progress", ProgressEvent {
    cluster_index: i,
    total_clusters: k,
});
```

The frontend displays a progress bar showing cluster N of K being processed.

## Module Layout (core/src/)

```
core/src/
├── lib.rs              # register commands, manage state
├── main.rs             # Tauri entry
├── database.rs         # SQLite connection + migration runner
├── file.rs             # get_file_metadata command
├── worksheet.rs        # worksheet CRUD commands
├── chunk.rs            # text splitting logic
├── ingest.rs           # parse + chunk + embed pipeline
├── retrieval.rs        # k-means clustering + centroid sampling
├── generation.rs       # SmolLM2 artifact generation
└── schema.rs           # shared structs (Chunk, Cluster, Artifact, etc.)
```
