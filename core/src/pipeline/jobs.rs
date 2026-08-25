//! Background pipeline job runner.
//!
//! One worksheet pipeline runs at a time (the gate keeps progress reporting
//! sane; generation itself no longer needs to serialize hardware). Provider
//! resolution happens per run so settings changes apply immediately.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sqlx::{Pool, Sqlite};
use tauri::{AppHandle, Emitter};

use crate::embedder::{self, EmbedderPool};
use crate::provider::{
    self, config::ProviderState, client::OpenAiClient, ArtifactBackend,
};
use crate::schema::{ArtifactType, PipelineStatus};

/// Live in-memory state of a running pipeline. The authoritative
/// `running`/`done`/`failed` marker lives on the worksheet row so it survives
/// a restart; this carries the transient progress the UI shows.
#[derive(Clone)]
struct RunningJob {
    phase: String,
    artifact_type: Option<ArtifactType>,
    done: usize,
    total: usize,
    types_done: usize,
    types_total: usize,
    requests_done: usize,
    tokens_in: u64,
    tokens_out: u64,
}

/// Registry of running pipelines plus the serialization gate.
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
    embedder_pool: Arc<EmbedderPool>,
    models_dir: PathBuf,
    providers: Arc<ProviderState>,
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
                types_total: ArtifactType::ALL.len(),
                requests_done: 0,
                tokens_in: 0,
                tokens_out: 0,
            },
        );
    }

    tauri::async_runtime::spawn(async move {
        let _guard = jobs.gate.lock().await;
        run_job(
            &app,
            &pool,
            &embedder_pool,
            &models_dir,
            &providers,
            &jobs,
            &worksheet_id,
        )
        .await;
    });
}

/// Persist `running` for any worksheet left mid-flight by a previous session
/// and restart its pipeline.
pub async fn resume_stale(
    app: AppHandle,
    pool: Pool<Sqlite>,
    embedder_pool: Arc<EmbedderPool>,
    models_dir: PathBuf,
    providers: Arc<ProviderState>,
    jobs: Arc<PipelineJobs>,
) {
    let rows = sqlx::query_as::<_, (String,)>(
        "SELECT id FROM worksheets WHERE pipeline_status = 'running'",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

    for (worksheet_id,) in rows {
        start_job(
            app.clone(),
            pool.clone(),
            embedder_pool.clone(),
            models_dir.clone(),
            providers.clone(),
            jobs.clone(),
            worksheet_id,
        );
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
            requests_done: running.requests_done,
            tokens_in: running.tokens_in,
            tokens_out: running.tokens_out,
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
            ArtifactType::ALL.len()
        } else {
            0
        },
        types_total: ArtifactType::ALL.len(),
        requests_done: 0,
        tokens_in: 0,
        tokens_out: 0,
        error: persisted_error,
    })
}

async fn run_job(
    app: &AppHandle,
    pool: &Pool<Sqlite>,
    embedder_pool: &Arc<EmbedderPool>,
    models_dir: &Path,
    providers: &Arc<ProviderState>,
    jobs: &Arc<PipelineJobs>,
    worksheet_id: &str,
) {
    let result = run_pipeline(app, pool, embedder_pool, models_dir, providers, jobs, worksheet_id).await;

    let (status, error) = match result {
        Ok(()) => (String::from("done"), None),
        Err(message) => {
            eprintln!("[pipeline] worksheet {worksheet_id} failed: {message}");
            (String::from("failed"), Some(message))
        }
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
            "types_total": ArtifactType::ALL.len(),
            "error": error,
        }),
    );
}

/// Build the generation backend from the current provider configuration.
pub fn resolve_backend(
    providers: &Arc<ProviderState>,
) -> Result<(Arc<dyn ArtifactBackend>, usize), String> {
    let config = providers.get();
    if !config.is_configured() {
        return Err(String::from(
            "No LLM provider is configured. Open Settings and add a provider.",
        ));
    }
    let api_key = if config.requires_api_key() {
        provider::config::load_api_key()?.filter(|key| !key.is_empty())
    } else {
        None
    };
    if config.requires_api_key() && api_key.is_none() {
        return Err(String::from(
            "The configured provider needs an API key. Open Settings and sign in again.",
        ));
    }
    let backend = Arc::new(OpenAiClient::new(&config.base_url, api_key, &config.model));
    Ok((backend, config.concurrency))
}

async fn run_pipeline(
    app: &AppHandle,
    pool: &Pool<Sqlite>,
    embedder_pool: &Arc<EmbedderPool>,
    models_dir: &Path,
    providers: &Arc<ProviderState>,
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

    // Resolve the cloud backend before doing any local work so a missing
    // configuration fails fast with an actionable message.
    let (backend, concurrency) = resolve_backend(providers)?;
    let embedder = embedder_pool.get(models_dir).await?;
    let tokenizer = embedder::bundled_tokenizer()?;

    let on_ingest = progress_sink(app, jobs, worksheet_id);
    super::process_files(pool, worksheet_id, &file_ids, tokenizer, embedder, Some(on_ingest)).await?;

    // Clear stale artifacts before regenerating so a re-run never duplicates.
    sqlx::query("DELETE FROM artifacts WHERE worksheet_id = ?")
        .bind(worksheet_id)
        .execute(pool)
        .await
        .map_err(|_| String::from("Failed to clear old artifacts"))?;

    set_phase(jobs, worksheet_id, "generating", None);

    let segments = super::load_segments(pool, worksheet_id).await?;
    let on_generate = generate_progress_sink(app, jobs, worksheet_id);
    let started = std::time::Instant::now();

    let (pending, telemetry) =
        super::generate_all(backend, concurrency, &segments, None, Some(on_generate)).await?;
    println!(
        "[pipeline] generated {} artifacts across {} requests (~{}k in / ~{}k out tokens) in {:?}",
        pending.len(),
        telemetry.requests,
        telemetry.tokens_in / 1000,
        telemetry.tokens_out / 1000,
        started.elapsed()
    );

    super::persist_artifacts(pool, worksheet_id, &pending).await?;

    Ok(())
}

fn progress_sink(
    app: &AppHandle,
    jobs: &Arc<PipelineJobs>,
    worksheet_id: &str,
) -> Box<dyn FnMut(usize, usize) + Send> {
    let app = app.clone();
    let jobs = jobs.clone();
    let worksheet_id = worksheet_id.to_string();
    Box::new(move |done, total| {
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

fn generate_progress_sink(
    app: &AppHandle,
    jobs: &Arc<PipelineJobs>,
    worksheet_id: &str,
) -> super::generate::ProgressFn {
    let app = app.clone();
    let jobs = jobs.clone();
    let worksheet_id = worksheet_id.to_string();
    Arc::new(move |tick| {
        {
            let mut running = jobs.jobs.lock().unwrap();
            if let Some(job) = running.get_mut(&worksheet_id) {
                job.done = tick.done;
                job.total = tick.total;
                job.types_done = tick.types_done;
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
) {
    let mut running = jobs.jobs.lock().unwrap();
    if let Some(job) = running.get_mut(worksheet_id) {
        job.phase = phase.to_string();
        job.artifact_type = artifact_type;
        job.done = 0;
        job.total = 0;
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
            "requests_done": job.requests_done,
            "tokens_in": job.tokens_in,
            "tokens_out": job.tokens_out,
            "error": null,
        }),
    );
}
