use tauri::State;

use crate::pipeline;
use crate::schema::{Artifact, ArtifactType, PipelineStatus};
use crate::AppState;

/// List the persisted artifacts of a worksheet for one artifact type,
/// optionally capped at `count` randomly-selected items.
#[tauri::command]
pub async fn get_artifacts(
    state: State<'_, AppState>,
    worksheet_id: String,
    artifact_type: ArtifactType,
    count: Option<i64>,
) -> Result<Vec<Artifact>, String> {
    pipeline::get_artifacts(&state.database, &worksheet_id, &artifact_type, count).await
}

/// Current pipeline status for a worksheet, merging live progress with the
/// persisted status column.
#[tauri::command]
pub async fn get_pipeline_status(
    state: State<'_, AppState>,
    worksheet_id: String,
) -> Result<PipelineStatus, String> {
    pipeline::get_status(&state.database, &state.jobs, &worksheet_id).await
}