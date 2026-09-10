use tauri::{AppHandle, State};

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
/// persisted status column. Also ensures an unfinished worksheet's pipeline is
/// resumed (starting on the first poll after the worksheet is opened) so
/// viewing the worksheet reflects live progress instead of a stale snapshot.
#[tauri::command]
pub async fn get_pipeline_status(
    app: AppHandle,
    state: State<'_, AppState>,
    worksheet_id: String,
) -> Result<PipelineStatus, String> {
    pipeline::resume_if_needed(
        app,
        &state.database,
        &state.settings,
        &state.jobs,
        &worksheet_id,
    )
    .await
}
