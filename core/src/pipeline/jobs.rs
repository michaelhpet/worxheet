use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sqlx::{Pool, Sqlite};
use tauri::{AppHandle, Emitter};

use super::generate::ProgressFn;
use crate::models::ModelPool;
use crate::schema::{ArtifactType, PipelineStatus};

const ARTIFACT_TYPES: [ArtifactType; 5] = [
    ArtifactType::MultipleChoiceQuiz,
    ArtifactType::EssayQuiz,
    ArtifactType::CompletionQuiz,
    ArtifactType::Summary,
    ArtifactType::MindMap,
];

/// Live in-memory state of a running pipeline. The authoritative
/// `running`/`done`/`failed` marker lives on the worksheet row so it survives a
/// restart; this only carries the transient progress the UI shows.
#[derive(Clone)]
struct RunningJob {
    phase: String,
    artifact_type: Option<ArtifactType>,
    done: usize,
    total: usize,
    types_done: usize,
    types_total: usize,
}

/// Registry of running pipelines plus the serialization gate that ensures only
/// one llama.cpp inference runs at a time.
pub struct PipelineJobs {
    jobs: Mutex<HashMap<String, RunningJob>>,
    gate: tokio::sync::Mutex<()>,
}

impl Default for PipelineJobs {
    fn default() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
            gate: tokio::sync::Mutex::new(()),
        }
    }
}

/// Register the worksheet as running and start its pipeline in the background.
pub fn start_job(
    app: AppHandle,
    pool: Pool<Sqlite>,
    models: Arc<ModelPool>,
    jobs: Arc<PipelineJobs>,
    worksheet_id: String,
) {
    {
        let mut running = jobs.jobs.lock().unwrap();
        running.insert(
            worksheet_id.clone(),
            RunningJob {
                phase: String::from("ingesting"),
                artifact_type: None,
                done: 0,
                total: 0,
                types_done: 0,
                types_total: ARTIFACT_TYPES.len(),
            },
        );
    }

    tauri::async_runtime::spawn(async move {
        let _guard = jobs.gate.lock().await;
        run_job(&app, &pool, &models, &jobs, &worksheet_id).await;
    });
}

/// Persist `running` for any worksheet left mid-flight by a previous session
/// and restart its pipeline.
pub async fn resume_stale(
    app: AppHandle,
    pool: Pool<Sqlite>,
    models: Arc<ModelPool>,
    jobs: Arc<PipelineJobs>,
) {
    let rows = sqlx::query_as::<_, (String,)>(
        "SELECT id FROM worksheets WHERE pipeline_status = 'running'",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

    for (worksheet_id,) in rows {
        start_job(app.clone(), pool.clone(), models.clone(), jobs.clone(), worksheet_id);
    }
}

/// Forget an in-memory job, e.g. when its worksheet is deleted.
pub fn remove_job(jobs: &Arc<PipelineJobs>, worksheet_id: &str) {
    jobs.jobs.lock().unwrap().remove(worksheet_id);
}

/// Merge live in-memory progress with the persisted worksheet status.
pub async fn get_status(
    pool: &Pool<Sqlite>,
    jobs: &Arc<PipelineJobs>,
    worksheet_id: &str,
) -> Result<PipelineStatus, String> {
    let (persisted, persisted_error): (String, Option<String>) =
        sqlx::query_as("SELECT pipeline_status, pipeline_error FROM worksheets WHERE id = ?")
            .bind(worksheet_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| String::from("Failed to fetch pipeline status"))?
            .ok_or_else(|| String::from("Worksheet not found"))?;

    if let Some(running) = jobs.jobs.lock().unwrap().get(worksheet_id) {
        return Ok(PipelineStatus {
            status: String::from("running"),
            phase: Some(running.phase.clone()),
            artifact_type: running
                .artifact_type
                .as_ref()
                .map(ArtifactType::to_db)
                .map(String::from),
            done: running.done,
            total: running.total,
            types_done: running.types_done,
            types_total: running.types_total,
            error: None,
        });
    }

    Ok(PipelineStatus {
        status: persisted.clone(),
        phase: None,
        artifact_type: None,
        done: 0,
        total: 0,
        types_done: if persisted == "done" {
            ARTIFACT_TYPES.len()
        } else {
            0
        },
        types_total: ARTIFACT_TYPES.len(),
        error: persisted_error,
    })
}

async fn run_job(
    app: &AppHandle,
    pool: &Pool<Sqlite>,
    models: &Arc<ModelPool>,
    jobs: &Arc<PipelineJobs>,
    worksheet_id: &str,
) {
    let result = run_pipeline(app, pool, models, jobs, worksheet_id).await;

    let (status, error) = match result {
        Ok(()) => (String::from("done"), None),
        Err(message) => (String::from("failed"), Some(message)),
    };

    sqlx::query("UPDATE worksheets SET pipeline_status = ?, pipeline_error = ? WHERE id = ?")
        .bind(&status)
        .bind(&error)
        .bind(worksheet_id)
        .execute(pool)
        .await
        .ok();

    jobs.jobs.lock().unwrap().remove(worksheet_id);

    let _ = app.emit(
        "pipeline-progress",
        serde_json::json!({
            "worksheet_id": worksheet_id,
            "status": status,
            "phase": null,
            "artifact_type": null,
            "done": 0,
            "total": 0,
            "types_done": 0,
            "types_total": ARTIFACT_TYPES.len(),
            "error": error,
        }),
    );
}

async fn run_pipeline(
    app: &AppHandle,
    pool: &Pool<Sqlite>,
    models: &Arc<ModelPool>,
    jobs: &Arc<PipelineJobs>,
    worksheet_id: &str,
) -> Result<(), String> {
    sqlx::query("UPDATE worksheets SET pipeline_status = 'running', pipeline_error = NULL WHERE id = ?")
        .bind(worksheet_id)
        .execute(pool)
        .await
        .map_err(|_| String::from("Failed to mark worksheet as running"))?;

    let file_ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM files WHERE worksheet_id = ? ORDER BY created_at")
            .bind(worksheet_id)
            .fetch_all(pool)
            .await
            .map_err(|_| String::from("Failed to load files"))?;

    if file_ids.is_empty() {
        return Err(String::from("Worksheet has no files to process."));
    }

    let on_ingest = progress_sink(app, jobs, worksheet_id);

    super::process_files(pool, models, worksheet_id, &file_ids, Some(on_ingest)).await?;

    // Clear stale artifacts before regenerating so a re-run never duplicates.
    sqlx::query("DELETE FROM artifacts WHERE worksheet_id = ?")
        .bind(worksheet_id)
        .execute(pool)
        .await
        .map_err(|_| String::from("Failed to clear old artifacts"))?;

    let mut types_done = 0usize;
    for artifact_type in ARTIFACT_TYPES {
        set_phase(
            jobs,
            worksheet_id,
            &String::from("generating"),
            Some(artifact_type.clone()),
            types_done,
        );

        let on_generate = progress_sink(app, jobs, worksheet_id);

        super::generate_artifacts(
            pool,
            models,
            worksheet_id,
            &artifact_type,
            None,
            Some(on_generate),
        )
        .await?;

        types_done += 1;
        set_phase(
            jobs,
            worksheet_id,
            &String::from("generating"),
            Some(artifact_type),
            types_done,
        );
    }

    Ok(())
}

fn progress_sink(
    app: &AppHandle,
    jobs: &Arc<PipelineJobs>,
    worksheet_id: &str,
) -> ProgressFn {
    let app = app.clone();
    let jobs = jobs.clone();
    let worksheet_id = worksheet_id.to_string();
    Arc::new(move |done, total| {
        {
            let mut running = jobs.jobs.lock().unwrap();
            if let Some(job) = running.get_mut(&worksheet_id) {
                job.done = done;
                job.total = total;
            }
        }
        emit_progress(&app, &jobs, &worksheet_id);
    })
}

fn set_phase(
    jobs: &Arc<PipelineJobs>,
    worksheet_id: &str,
    phase: &str,
    artifact_type: Option<ArtifactType>,
    types_done: usize,
) {
    let mut running = jobs.jobs.lock().unwrap();
    if let Some(job) = running.get_mut(worksheet_id) {
        job.phase = phase.to_string();
        job.artifact_type = artifact_type;
        job.done = 0;
        job.total = 0;
        job.types_done = types_done;
    }
}

fn emit_progress(app: &AppHandle, jobs: &Arc<PipelineJobs>, worksheet_id: &str) {
    let snapshot = jobs.jobs.lock().unwrap().get(worksheet_id).cloned();
    let Some(job) = snapshot else {
        return;
    };
    let _ = app.emit(
        "pipeline-progress",
        serde_json::json!({
            "worksheet_id": worksheet_id,
            "status": "running",
            "phase": job.phase,
            "artifact_type": job.artifact_type.as_ref().map(ArtifactType::to_db),
            "done": job.done,
            "total": job.total,
            "types_done": job.types_done,
            "types_total": job.types_total,
            "error": null,
        }),
    );
}