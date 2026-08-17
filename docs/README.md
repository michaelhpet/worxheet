# Worxheet Documentation

Local-first RAG application: study material is uploaded, parsed, chunked, embedded, topic-clustered, and turned into grammar-constrained artifacts (MCQs, essays, fill-in-the-blank, summaries, mind-maps) entirely on-device.

## Diagrams (Mermaid)

| Page | What it shows |
|------|---------------|
| [diagrams/architecture.md](diagrams/architecture.md) | Tauri process structure: React frontend ⇄ IPC ⇄ Rust core ⇄ SQLite, plus the GGUF model layer and the process-wide llama.cpp backend. |
| [diagrams/sequence.md](diagrams/sequence.md) | End-to-end sequence from worksheet creation through ingestion to artifact generation and viewing, including progress events. |
| [diagrams/ingestion.md](diagrams/ingestion.md) | Zoomed into the ingest path: parse → chunk → store → embed → cluster. |
| [diagrams/generation.md](diagrams/generation.md) | Zoomed into the generation path: per-cluster units → grammar-constrained generation → artifact assembly. |
| [diagrams/data-model.md](diagrams/data-model.md) | SQLite entity-relationship model (worksheets, files, chunks, clusters, artifacts). |

## The pipeline at a glance

```
Upload ─► Parse ─► Chunk ─► Store ─► Embed ─► Cluster ─► Generate ─► Persist
```

Creating a worksheet automatically starts a background job
(`core/src/pipeline/jobs.rs`) that runs the whole pipeline end-to-end — the
frontend only waits for it to finish and then shows the artifacts. The heavy
compute (`parse_file`, `chunk_text`, embedding, HDBSCAN clustering, LLM
generation) is dispatched off the async runtime via `spawn_blocking`.
Persistence is SQLite; vectors live as BLOB columns rather than in a vector
database.

- **Rust core** (`core/src/`) — all AI inference, document extraction, storage,
  and the background job runner. See [PIPELINE.md](PIPELINE.md) for a prose
  walkthrough of every stage.
- **Frontend** (`app/`) — React UI that creates worksheets and renders live
  pipeline progress (`usePipelineStatus`, polling `get_pipeline_status`) plus
  the generated artifacts.

## Related docs

- [ARCHITECTURE.md](ARCHITECTURE.md) — high-level architecture and design decisions.
- [PIPELINE.md](PIPELINE.md) — step-by-step RAG pipeline implementation.
- [DEPENDENCIES.md](DEPENDENCIES.md) — third-party dependency rationale.
- [ROADMAP.md](ROADMAP.md) — planned work.
