# Data Model (SQLite)

Entity-relationship model for the `database.sqlite` file. All IDs are ULIDs.
Embeddings are stored as little-endian float32 BLOBs (`384 × f32 = 1536` bytes
for bge-small) rather than in a vector database.

```mermaid
erDiagram
    WORKSHEETS ||--o{ FILES : "contains"
    WORKSHEETS ||--o{ CHUNKS : "owns"
    WORKSHEETS ||--o{ ARTIFACTS : "has"
    FILES ||--o{ CHUNKS : "parsed into"
    WORKSHEETS ||--o{ CLUSTERS : "topic groups"
    CHUNKS }o--o{ CLUSTERS : "assigned to (cluster_index)"

    WORKSHEETS {
        text id PK "ULID"
        text name
        timestamp created_at
        timestamp updated_at
    }

    FILES {
        text id PK "ULID"
        text worksheet_id FK "ON DELETE CASCADE"
        text path
        text name
        text extension
        integer size
        text status "uploaded | parsing | parsed"
        timestamp created_at
    }

    CHUNKS {
        text id PK "ULID"
        text worksheet_id FK "ON DELETE CASCADE"
        text file_id FK "ON DELETE CASCADE"
        integer position "document order"
        text text
        blob embedding "f32 LE, 1536 bytes, NULL until embedded"
        integer cluster_index "HDBSCAN label or NULL (noise)"
        timestamp created_at
    }

    CLUSTERS {
        text worksheet_id FK "composite PK"
        integer cluster_index "composite PK"
        blob centroid "f32 LE, normalized"
        integer size "member count"
        timestamp created_at
    }

    ARTIFACTS {
        text id PK "ULID"
        text worksheet_id FK "ON DELETE CASCADE"
        text artifact_type "MultipleChoiceQuiz | EssayQuiz | CompletionQuiz | Summary | MindMap"
        text source "comma-joined source chunk ids"
        text content "JSON payload"
        timestamp created_at
    }
```

## Notes on the model

- **Cascades** delete a worksheet's files, chunks, clusters, and artifacts when
  the worksheet is removed (`ON DELETE CASCADE` on `worksheet_id`/`file_id`).
- **Indexes** (from migrations `20260419163341_schema.sql`,
  `20260630000001_pipeline.sql`, `20260811000000_clusters.sql`):
  - `idx_files_worksheet ON files(worksheet_id)`
  - `idx_chunks_worksheet ON chunks(worksheet_id)`
  - `idx_chunks_worksheet_position ON chunks(worksheet_id, position)`
  - `idx_artifacts_worksheet_type ON artifacts(worksheet_id, artifact_type)`
  - `idx_clusters_worksheet ON clusters(worksheet_id)`
- **Chunk ↔ Cluster** is many-to-many in spirit, but only the cluster side is
  persisted as a table (one centroid per cluster). Each chunk carries its
  cluster label denormalized in `chunks.cluster_index`, which the generation
  path uses to group units.
- **Embedding BLOBs** are decoded back to `Vec<f32>` by
  `retrieval.rs::bytes_to_embedding` for cosine retrieval and clustering; encoded
  by `embedding_to_bytes`. The cluster centroids are stored the same way and
  are L2-normalized.
