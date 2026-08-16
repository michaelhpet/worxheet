use tauri::State;

use crate::pipeline::{self, FileInfo, RetrievedChunk};
use crate::schema::{Artifact, ArtifactType, Chunk};
use crate::AppState;
use crate::generation::GenerationParams;

/// List the files attached to a worksheet (used by the frontend to obtain file
/// IDs before running the ingestion pipeline).
#[tauri::command]
pub async fn get_files(
    state: State<'_, AppState>,
    worksheet_id: String,
) -> Result<Vec<FileInfo>, String> {
    pipeline::get_files(&state.database, &worksheet_id).await
}

/// Parse, chunk, and store the given files of a worksheet. Emits
/// `ingestion-progress` events as each file completes.
#[tauri::command]
pub async fn process_files(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    worksheet_id: String,
    file_ids: Vec<String>,
) -> Result<Vec<Chunk>, String> {
    pipeline::process_files(
        Some(&app),
        &state.database,
        &state.models,
        &worksheet_id,
        &file_ids,
    )
    .await
}

/// Embed every chunk of a worksheet that does not yet have an embedding and
/// store the vectors as BLOBs. Returns the number of chunks embedded.
#[tauri::command]
pub async fn embed_worksheet(
    state: State<'_, AppState>,
    worksheet_id: String,
) -> Result<usize, String> {
    pipeline::embed_worksheet(&state.database, &state.models, &worksheet_id).await
}

/// RAG retrieval: embed the query and return the top-k most similar chunks.
#[tauri::command]
pub async fn retrieve_chunks(
    state: State<'_, AppState>,
    worksheet_id: String,
    query: String,
    top_k: Option<usize>,
) -> Result<Vec<RetrievedChunk>, String> {
    pipeline::retrieve_chunks(&state.database, &state.models, &worksheet_id, &query, top_k).await
}

/// Generate artifacts for a worksheet.
#[tauri::command]
pub async fn generate_artifacts(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    worksheet_id: String,
    artifact_type: ArtifactType,
    params: Option<GenerationParams>,
) -> Result<Vec<Artifact>, String> {
    pipeline::generate_artifacts(
        Some(&app),
        &state.database,
        &state.models,
        &worksheet_id,
        &artifact_type,
        params,
    )
    .await
}

/// List the persisted artifacts of a worksheet.
#[tauri::command]
pub async fn get_artifacts(
    state: State<'_, AppState>,
    worksheet_id: String,
) -> Result<Vec<Artifact>, String> {
    pipeline::get_artifacts(&state.database, &worksheet_id).await
}