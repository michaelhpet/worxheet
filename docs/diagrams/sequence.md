# End-to-End Sequence

The user journey through the RAG pipeline. Creating a worksheet kicks off an
automatic background job that runs ingest → embed → cluster → generate with no
further user action. The frontend column is the React UI; the backend column is
the Rust core invoked over Tauri IPC. Progress is streamed back through a single
`pipeline-progress` event and polled via `get_pipeline_status`.

```mermaid
sequenceDiagram
    autonumber
    actor User
    participant UI as React Frontend
    participant Rust as Rust Core (pipeline/jobs.rs)
    participant SQLite as SQLite
    participant Models as ModelPool (GGUF)

    rect rgb(238, 244, 248)
    Note over User,Models: PIPELINE (automatic)
    User->>UI: Create worksheet + pick files (dialog / drag-drop)
    UI->>Rust: create_worksheet(name, files[])
    Rust->>SQLite: INSERT worksheets (status='running') + files
    Rust-->>UI: worksheet (id)
    Rust->>Rust: start_job → spawn pipeline
    Rust-->>UI: emit pipeline-progress {status:running, phase:ingesting}
    loop each file
        Rust->>Rust: parse_file(path, ext) → text (pdf_oxide/office_oxide)
        Rust->>Rust: chunk_text(text, 512, 128, tokenizer)
        Rust->>SQLite: INSERT chunks (position, text)
        Rust-->>UI: emit pipeline-progress {phase:ingesting, done, total}
    end
    Rust->>Rust: embed_missing_chunks()
    Rust->>Models: embedder() → embed(texts)
    Rust->>SQLite: UPDATE chunks SET embedding=BLOB
    Rust->>Rust: rebuild_clusters() → HDBSCAN
    Rust->>SQLite: UPSERT clusters + chunks.cluster_index
    Rust->>SQLite: DELETE FROM artifacts (clear stale)
    Rust-->>UI: emit pipeline-progress {phase:generating, types_done, types_total}
    loop each artifact type (MCQ, Essay, Completion, Summary, MindMap)
        Rust->>Rust: generate_artifacts → cluster_contexts → per-cluster units
        loop each unit (one per topic cluster)
            Rust->>Rust: build prompt (system + user message)
            Rust->>Models: generator() → grammar-constrained generate
            alt invalid / truncated output
                Rust->>Models: retry with doubled (capped) max_tokens
            end
            Rust-->>UI: emit pipeline-progress {done, total}
        end
        Rust->>Rust: assemble_artifacts (question split | summary/mindmap merge)
        loop each artifact
            Rust->>SQLite: INSERT artifacts
        end
    end
    Rust->>SQLite: UPDATE worksheets SET status='done'
    Rust-->>UI: emit pipeline-progress {status:done}
    end

    rect rgb(244, 240, 238)
    Note over User,Models: VIEW
    UI->>Rust: get_pipeline_status(worksheet_id)
    UI->>Rust: get_artifacts(worksheet_id)
    Rust-->>UI: artifacts[]
    UI-->>User: Artifacts displayed
    end
```

## Interaction stages captured from the frontend

- **File selection**: `FilesUploader` (`app/components/files-uploader.tsx`) uses
  the Tauri dialog plugin (`open` with a document filter) and drag-and-drop
  (`TauriEvent.DRAG_DROP`), filtering to `SUPPORTED_EXTENSIONS`.
- **Worksheet creation**: `CreateWorksheetDialog`
  (`app/components/create-worksheet-dialog.tsx`) collects a name + file paths,
  calls `create_worksheet` (which starts the pipeline job), then navigates to
  `/worksheets/$id`.
- **Progress**: `usePipelineStatus` (`app/data/pipeline.ts`) polls
  `get_pipeline_status` every ~1.2s while the status is `running` and renders
  the phase (`ingesting`/`generating`), file/unit counts, and per-type progress.
- **Artifacts**: `ArtifactsPanel` (`app/components/workspace/artifacts-panel.tsx`)
  lists and filters artifact cards; `get_artifacts` populates them once the job
  finishes.