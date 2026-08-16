# Architecture Diagram

The entire application runs inside a single Tauri process. The React frontend
drives the Rust backend over async IPC; the backend owns document extraction,
embeddings, clustering, generation, and SQLite persistence. There are no cloud
dependencies.

```mermaid
flowchart TB
    subgraph TAURI["Tauri Process"]
        subgraph FE["Frontend (React)"]
            Router["TanStack Router<br/>routes + dialogs"]
            Query["TanStack Query<br/>hooks (data/*)"]
            Progress["Progress listeners<br/>(event listeners)"]
            Router --> Query
            Router --> Progress
        end

        subgraph IPC["IPC Layer"]
            Commands["invoke handler (lib.rs)<br/>tauri commands"]
            Events["Event emitter<br/>ingestion-progress / generation-progress / model-download"]
        end

        subgraph BE["Backend (Rust /core)"]
            subgraph COMMANDS["commands/ (tauri command wrappers)"]
                CMD["process_files / embed_worksheet<br/>retrieve_chunks / generate_artifacts"]
            end

            subgraph PIPELINE["pipeline.rs (testable cores)"]
                PF["process_files<br/>parse + chunk + store + embed + cluster"]
                EW["embed_worksheet"]
                RC["retrieve_chunks"]
                GA["generate_artifacts"]
            end

            subgraph MODULES["Processing modules"]
                Ingest["ingest.rs<br/>parse_file (pdf_oxide / office_oxide)"]
                Chunk["chunk.rs<br/>chunk_text (512/128 tokens)"]
                Embed["embed.rs<br/>Embedder (bge-small-en-v1.5, 384d)"]
                Cluster["cluster.rs<br/>HDBSCAN assign_clusters"]
                Retrieval["retrieval.rs<br/>cosine top-k"]
                Generation["generation.rs<br/>Generator (SmolLM2-360M)"]
            end

            subgraph MODELS["Model layer"]
                Pool["models.rs ModelPool<br/>lazy download + cache"]
                Tokenizer["tokenizer.json<br/>(chunking)"]
                EmbedGGUF["bge-small-en-v1.5-q8_0.gguf<br/>(~27MB)"]
                GenGGUF["smollm2-360m-instruct-q8_0.gguf<br/>(~365MB)"]
                LLM["llm.rs<br/>process-wide LlamaBackend (OnceLock)"]
            end
        end

        DB[("SQLite<br/>database.sqlite")]
    end

    Router -- "invoke(...)" --> Commands
    Commands --> PIPELINE
    Events -- "emit" --> Progress
    PIPELINE -- "calls" --> MODULES
    MODULES -- "reads/writes" --> DB
    Pool --> EmbedGGUF
    Pool --> GenGGUF
    Pool --> Tokenizer
    Embed --> EmbedGGUF
    Generation --> GenGGUF
    Chunk --> Tokenizer
    Embed --> LLM
    Generation --> LLM
    Ingest -. "reads source files on disk" .-> FS["user's PDF/PPTX/DOCX files"]
```

## Moving parts

- **Frontend (React)** drives the pipeline via `invoke` on Tauri commands and
  renders live state from events (`usePipelineProgress` in `app/data/progress.ts`).
- **commands/** is the thin IPC layer: each `#[tauri::command]` wrapper extracts
  `State` (database + models) and delegates to a plain-named core.
- **pipeline.rs** is the orchestration layer: testable cores that take a pool
  and model pool directly (`core/src/pipeline.rs`). Heavy work is wrapped in
  `tauri::async_runtime::spawn_blocking` so the event loop stays responsive.
- **Processing modules** are the pipeline stages. `ingest.rs` extracts text,
  `chunk.rs` splits it, `embed.rs` vectorizes, `cluster.rs` discovers topics,
  `retrieval.rs` scores similarity, and `generation.rs` produces structured JSON.
- **Model layer** (`models.rs`) lazily downloads the three GGUF/tokenizer
  artifacts from Hugging Face on first use into the app data `models/` dir and
  caches the loaded models for the process lifetime. Both models share the
  single llama.cpp `LlamaBackend` created once in `llm.rs`.
- **SQLite** is the single store: worksheets, files, chunks (with BLOB
  embeddings), clusters (centroids), and artifacts.
