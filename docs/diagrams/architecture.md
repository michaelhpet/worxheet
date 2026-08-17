# Architecture Diagram

The entire application runs inside a single Tauri process. The React frontend
drives the Rust backend over async IPC; the backend owns document extraction,
embeddings, clustering, generation, job scheduling, and SQLite persistence.
There are no cloud dependencies.

```mermaid
flowchart TB
    subgraph TAURI["Tauri Process"]
        subgraph FE["Frontend (React)"]
            Router["TanStack Router<br/>routes + dialogs"]
            Query["TanStack Query<br/>hooks (data/*)"]
            Pipeline["usePipelineStatus<br/>poll get_pipeline_status"]
            Router --> Query
            Router --> Pipeline
        end

        subgraph IPC["IPC Layer"]
            Commands["invoke handler (lib.rs)<br/>tauri commands"]
            Events["Event emitter<br/>pipeline-progress / model-download"]
        end

        subgraph BE["Backend (Rust /core)"]
            subgraph COMMANDS["commands/ (tauri command wrappers)"]
                CMD["create_worksheet / delete_worksheet<br/>get_artifacts / get_pipeline_status"]
            end

            subgraph JOBS["pipeline/jobs.rs (background runner)"]
                PJ["PipelineJobs<br/>start_job / resume_stale / get_status"]
                PF["process_files<br/>parse + chunk + store + embed + cluster"]
                GA["generate_artifacts<br/>all five artifact types"]
            end

            subgraph PIPELINE["pipeline/ (processing modules)"]
                Ingest["ingest.rs<br/>parse_file (pdf_oxide / office_oxide)<br/>chunk_text (512/128 tokens)"]
                Embed["embed.rs<br/>Embedder (bge-small-en-v1.5, 384d)<br/>BLOB encode/decode"]
                Cluster["cluster.rs<br/>HDBSCAN assign_clusters<br/>cluster_contexts + rebuild_clusters"]
                Generation["generate.rs<br/>Generator (SmolLM2-360M)<br/>assemble_artifacts"]
            end

            subgraph MODELS["Model layer"]
                Pool["models.rs ModelPool + Embedder + Generator<br/>lazy download + cache"]
                Tokenizer["tokenizer.json<br/>(chunking)"]
                EmbedGGUF["bge-small-en-v1.5-q8_0.gguf<br/>(~27MB)"]
                GenGGUF["smollm2-360m-instruct-q8_0.gguf<br/>(~365MB)"]
            end
        end

        DB[("SQLite<br/>database.sqlite")]
    end

    Router -- "invoke(...)" --> Commands
    Commands --> JOBS
    Events -- "emit" --> Pipeline
    JOBS --> PIPELINE
    PIPELINE -- "reads/writes" --> DB
    Pool --> EmbedGGUF
    Pool --> GenGGUF
    Pool --> Tokenizer
    Embed --> EmbedGGUF
    Generation --> GenGGUF
    Ingest --> Tokenizer
    Ingest -. "reads source files on disk" .-> FS["user's PDF/PPTX/DOCX files"]
```

## Moving parts

- **Frontend (React)** drives the pipeline via `invoke` on Tauri commands —
  creating a worksheet is the entire interaction — and renders live state by
  polling `get_pipeline_status` (`usePipelineStatus` in `app/data/pipeline.ts`).
- **commands/** is the thin IPC layer: each `#[tauri::command]` wrapper extracts
  `State` (database + models + jobs) and delegates to a plain-named core.
- **pipeline/jobs.rs** is the orchestration layer: `PipelineJobs` schedules one
  pipeline at a time (a tokio gate serializes inference), streams progress,
  persists `pipeline_status`, and re-runs stale jobs on startup
  (`resume_stale`). Heavy work is wrapped in `tauri::async_runtime::spawn_blocking`
  so the event loop stays responsive.
- **pipeline/ modules** are the pipeline stages. `ingest.rs` extracts + chunks
  text, `embed.rs` vectorizes, `cluster.rs` discovers topics, and `generate.rs`
  produces structured JSON.
- **Model layer** (`models.rs`) lazily downloads the three GGUF/tokenizer
  artifacts from Hugging Face on first use into the app data `models/` dir and
  caches the loaded models for the process lifetime. Both models share the
  single llama.cpp `LlamaBackend` created once in `models.rs`.
- **SQLite** is the single store: worksheets (with `pipeline_status`),
  files, chunks (with BLOB embeddings), clusters (centroids), and artifacts.