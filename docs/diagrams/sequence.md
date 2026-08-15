# End-to-End Sequence

The user journey through the RAG pipeline, split into the **ingest** phase and
the **generate** phase. The frontend column is the React UI; the backend column
is the Rust core invoked over Tauri IPC. Progress is streamed back through
emitted events while the command is still running.

```mermaid
sequenceDiagram
    autonumber
    actor User
    participant UI as React Frontend
    participant Rust as Rust Core (pipeline.rs)
    participant SQLite as SQLite
    participant Models as ModelPool (GGUF)

    rect rgb(238, 244, 248)
    Note over User,Models: PHASE 1 — INGESTION
    User->>UI: Create worksheet + pick files (dialog / drag-drop)
    UI->>Rust: create_worksheet(name, files[])
    Rust->>SQLite: INSERT worksheets + files (status='uploaded')
    Rust-->>UI: worksheet (id)
    UI->>Rust: get_files(worksheet_id) → list with status
    UI->>Rust: process_files(worksheet_id, file_ids[])
    Rust->>Models: tokenizer()
    loop each file
        Rust->>SQLite: UPDATE files SET status='parsing'
        Rust->>Rust: parse_file(path, ext) → text (pdf_oxide/office_oxide)
        Rust->>Rust: chunk_text(text, 512, 128, tokenizer)
        Rust->>SQLite: INSERT chunks (position, text)
        Rust->>SQLite: UPDATE files SET status='parsed'
        Rust-->>UI: emit ingestion-progress {done, total}
    end
    Rust->>Rust: embed_missing_chunks()
    Rust->>Models: embedder() → embed(texts)
    Rust->>SQLite: UPDATE chunks SET embedding=BLOB
    Rust->>Rust: rebuild_clusters() → HDBSCAN
    Rust->>SQLite: UPSERT clusters + chunks.cluster_index
    Rust-->>UI: chunks[]
    end

    rect rgb(244, 240, 238)
    Note over User,Models: PHASE 2 — GENERATION
    User->>UI: Open generate dialog, pick artifact type
    UI->>Rust: generate_artifacts(worksheet_id, type, params?)
    Rust-->>UI: emit generation-progress {done:0, total:0} (reset)
    Rust->>SQLite: SELECT chunks (position, text, cluster_index, embedding)
    Rust->>Rust: auto-embed missing vectors
    Rust->>Rust: cluster_contexts(labels, positions) → per-cluster units
    Rust-->>UI: emit generation-progress {total: units}
    loop each unit (one per topic cluster)
        Rust->>Rust: build prompt (system + user message)
        Rust->>Models: generator() → grammar-constrained generate
        alt invalid / truncated output
            Rust->>Models: retry with doubled (capped) max_tokens
        end
        Rust-->>UI: emit generation-progress {done: index+1}
    end
    Rust->>Rust: assemble_artifacts (question split | summary/mindmap merge)
    loop each artifact
        Rust->>SQLite: INSERT artifacts
    end
    Rust-->>UI: artifacts[]
    UI->>Rust: get_artifacts(worksheet_id) → render cards
    UI-->>User: Artifacts displayed
    end
```

## Interaction stages captured from the frontend

- **File selection**: `FilesUploader` (`app/components/files-uploader.tsx`) uses
  the Tauri dialog plugin (`open` with a document filter) and drag-and-drop
  (`TauriEvent.DRAG_DROP`), filtering to `SUPPORTED_EXTENSIONS`.
- **Worksheet creation**: `CreateWorksheetDialog`
  (`app/components/create-worksheet-dialog.tsx`) collects a name + file paths and
  calls `create_worksheet`, then navigates to `/worksheets/$id`.
- **Processing**: `FilesPanel` (`app/components/workspace/files-panel.tsx`)
  invokes `process_files` on un-parsed files and renders
  `ingestion-progress` + `model-download` progress bars.
- **Generation**: `GenerateDialog` (`app/components/workspace/generate-dialog.tsx`)
  picks an artifact type and optional advanced sampling params, invokes
  `generate_artifacts`, and renders `generation-progress` while busy.
- **Artifacts**: `ArtifactsPanel` (`app/components/workspace/artifacts-panel.tsx`)
  lists and filters artifact cards; `get_artifacts` populates them.

All hooks live in `app/data/*` (`files.tsx`, `artifacts.tsx`, `progress.ts`,
`model-downloads.ts`).
