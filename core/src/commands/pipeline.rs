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
    pipeline::get_artifacts(&state.database, &worksheet_id, &artifact_type, count)
        .await
        .map_err(|e| e.to_string())
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
    .map_err(|e| e.to_string())
}

/// Restart a worksheet's pipeline, e.g. after the user fixes the cause of a
/// failure (missing provider config, etc.) or after a user-requested stop.
/// Resumes from already-persisted chunks/artifacts. Idempotent: if a job for
/// the worksheet is already live, it is a no-op.
#[tauri::command]
pub async fn retry_pipeline(
    app: AppHandle,
    state: State<'_, AppState>,
    worksheet_id: String,
) -> Result<(), String> {
    pipeline::start_job(
        app,
        state.database.clone(),
        state.settings.clone(),
        state.jobs.clone(),
        worksheet_id,
    );
    Ok(())
}

/// Delete a worksheet's artifacts of the given types and re-run the pipeline
/// restricted to those types, e.g. after fixing the cause of a per-type
/// failure. Ingestion still processes pending files; other artifact types are
/// untouched. Fails when a job for the worksheet is already running so a
/// live run is never clobbered.
#[tauri::command]
pub async fn regenerate_artifacts(
    app: AppHandle,
    state: State<'_, AppState>,
    worksheet_id: String,
    artifact_types: Vec<ArtifactType>,
) -> Result<(), String> {
    if artifact_types.is_empty() {
        return Err(String::from("Select at least one artifact type to re-generate."));
    }
    if pipeline::is_running(&state.jobs, &worksheet_id) {
        return Err(String::from(
            "Pipeline is already running for this worksheet. Stop it first.",
        ));
    }
    pipeline::delete_artifacts_of_types(&state.database, &worksheet_id, &artifact_types)
        .await
        .map_err(|e| e.to_string())?;
    pipeline::start_job_for_types(
        app,
        state.database.clone(),
        state.settings.clone(),
        state.jobs.clone(),
        worksheet_id,
        Some(artifact_types),
    );
    Ok(())
}

/// Stop a worksheet's pipeline on demand. Only the named worksheet is
/// affected; other running worksheets continue untouched. Finished artifacts
/// stay persisted so a later `retry_pipeline` resumes the remainder.
/// Idempotent: returns `false` when nothing was running.
#[tauri::command]
pub async fn stop_pipeline(
    app: AppHandle,
    state: State<'_, AppState>,
    worksheet_id: String,
) -> Result<bool, String> {
    Ok(pipeline::stop_job(&app, &state.database, &state.jobs, &worksheet_id).await)
}
