//! Background pipeline job runner.
//!
//! Each worksheet pipeline runs on its own background task; progress reporting
//! is keyed by worksheet so runs do not need to serialize. Provider resolution
//! happens per run so settings changes apply immediately.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sqlx::{Pool, Sqlite};
use tauri::{AppHandle, Emitter};

use crate::provider::{self, client::OpenAiClient, ArtifactBackend};
use crate::schema::{ArtifactType, PipelineStatus, TypeProgress};
use crate::settings::SettingsState;

use super::segment;

/// Live in-memory state of a running pipeline. The authoritative
/// `running`/`done`/`failed` marker lives on the worksheet row so it survives
/// a restart; this carries the transient progress the UI shows.
#[derive(Clone)]
struct RunningJob {
    phase: String,
    artifact_type: Option<ArtifactType>,
    done: usize,
    total: usize,
    /// Per-artifact-type unit progress in the current phase.
    per_type: Vec<TypeProgress>,
    types_done: usize,
    types_total: usize,
    requests_done: usize,
    tokens_in: u64,
    tokens_out: u64,
}

/// Registry of in-memory running pipelines.
pub struct PipelineJobs {
    jobs: Mutex<HashMap<String, RunningJob>>,
}

impl Default for PipelineJobs {
    fn default() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
        }
    }
}

/// Register the worksheet as running and start its pipeline in the background.
/// Returns `false` (and does nothing) if a job for the worksheet is already
/// live, so concurrent callers cannot start duplicate pipelines.
pub fn start_job(
    app: AppHandle,
    pool: Pool<Sqlite>,
    settings: Arc<SettingsState>,
    jobs: Arc<PipelineJobs>,
    worksheet_id: String,
) -> bool {
    let started = {
        let mut running = jobs.jobs.lock().unwrap();
        if running.contains_key(&worksheet_id) {
            false
        } else {
            running.insert(
                worksheet_id.clone(),
                RunningJob {
                    phase: String::from("ingesting"),
                    artifact_type: None,
                    done: 0,
                    total: 0,
                    per_type: Vec::new(),
                    types_done: 0,
                    types_total: ArtifactType::ALL.len(),
                    requests_done: 0,
                    tokens_in: 0,
                    tokens_out: 0,
                },
            );
            true
        }
    };

    if !started {
        return false;
    }

    tauri::async_runtime::spawn(async move {
        run_job(&app, &pool, &settings, &jobs, &worksheet_id).await;
    });
    true
}

/// Persist `running` for any worksheet left mid-flight by a previous session
/// and restart its pipeline.
pub async fn resume_stale(
    app: AppHandle,
    pool: Pool<Sqlite>,
    settings: Arc<SettingsState>,
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
            settings.clone(),
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
            types: running.per_type.clone(),
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
        types: Vec::new(),
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

/// Ensure a worksheet's pipeline is running, resuming from unchunked or
/// ungenerated parts. Safe to call on every status poll: if a live job exists
/// or the worksheet is already `done`, it is a no-op. Returns the current
/// status so callers can render immediately after a resume is triggered.
pub async fn resume_if_needed(
    app: AppHandle,
    pool: &Pool<Sqlite>,
    settings: &Arc<SettingsState>,
    jobs: &Arc<PipelineJobs>,
    worksheet_id: &str,
) -> Result<PipelineStatus, String> {
    let live = jobs.jobs.lock().unwrap().contains_key(worksheet_id);
    if live {
        return get_status(pool, jobs, worksheet_id).await;
    }

    let persisted: Option<(String,)> =
        sqlx::query_as("SELECT pipeline_status FROM worksheets WHERE id = ?")
            .bind(worksheet_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| String::from("Failed to fetch pipeline status"))?;

    let Some((status,)) = persisted else {
        return Err(String::from("Worksheet not found"));
    };

    if status == "done" {
        return get_status(pool, jobs, worksheet_id).await;
    }

    // A live job may have been registered racing with this read; start_job is
    // idempotent, so only the first caller actually launches the pipeline.
    start_job(
        app,
        pool.clone(),
        settings.clone(),
        jobs.clone(),
        worksheet_id.to_string(),
    );
    get_status(pool, jobs, worksheet_id).await
}

async fn run_job(
    app: &AppHandle,
    pool: &Pool<Sqlite>,
    settings: &Arc<SettingsState>,
    jobs: &Arc<PipelineJobs>,
    worksheet_id: &str,
) {
    let result = run_pipeline(app, pool, settings, jobs, worksheet_id).await;

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
            "types": [],
            "types_done": 0,
            "types_total": ArtifactType::ALL.len(),
            "error": error,
        }),
    );
}

/// Build the generation backend from the current provider configuration.
fn resolve_backend(
    settings: &Arc<SettingsState>,
) -> Result<(Arc<dyn ArtifactBackend>, usize), String> {
    let provider = settings.get().provider;
    let config = provider.active_config();
    if !config.is_configured() {
        return Err(String::from(
            "No LLM provider is configured. Open Settings and add a provider.",
        ));
    }
    // Key presence is the only signal: absent key → no Authorization header.
    let api_key = provider::config::load_api_key(&provider.active)?.filter(|key| !key.is_empty());
    let backend = OpenAiClient::new(&config.base_url, api_key, &config.model)
        .with_reasoning_effort(
            config
                .disable_thinking
                .then(|| String::from("none")),
        );
    Ok((Arc::new(backend), config.concurrency))
}

async fn run_pipeline(
    app: &AppHandle,
    pool: &Pool<Sqlite>,
    settings: &Arc<SettingsState>,
    jobs: &Arc<PipelineJobs>,
    worksheet_id: &str,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE worksheets SET pipeline_status = 'running', pipeline_error = NULL WHERE id = ?",
    )
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

    // Resume from unchunked parts: only ingest files that have no chunks yet.
    let pending_files = super::unchunked_files(pool, worksheet_id, &file_ids).await?;

    // Resolve the cloud backend before doing any local work so a missing
    // configuration fails fast with an actionable message.
    let (backend, concurrency) = resolve_backend(settings)?;
    let tokenizer = segment::bundled_tokenizer()?;
    let logs = Arc::new(
        crate::logging::RunLogs::new()
            .ok_or_else(|| String::from("Failed to resolve log directory"))?,
    );

    let on_ingest = progress_sink(app, jobs, worksheet_id);
    let start_position = super::next_segment_position(pool, worksheet_id).await?;

    // Reuse already-ingested chunks for any file whose content matches an
    // existing source, rather than re-parsing/segmenting a duplicate.
    let (to_parse, start_position) =
        super::reuse_chunks(pool, worksheet_id, &pending_files, start_position).await?;

    let tokenizer = Arc::new(tokenizer);
    super::process_files(
        pool,
        worksheet_id,
        &to_parse,
        start_position,
        tokenizer,
        Some(on_ingest),
        Some(logs.clone()),
    )
    .await?;

    set_phase(jobs, worksheet_id, "generating", None);

    let segments = super::load_segments(pool, worksheet_id).await?;
    let existing = super::load_existing_artifacts(pool, worksheet_id).await?;
    let on_generate = generate_progress_sink(app, jobs, worksheet_id);
    let started = std::time::Instant::now();

    // Per-item quiz artifacts persist synchronously as each unit completes so
    // an interruption keeps finished parts. Merged types (Summary/MindMap) are
    // returned below and persisted once in `pending`.
    let on_persist = {
        let pool = pool.clone();
        let worksheet_id = worksheet_id.to_string();
        Arc::new(move |artifacts: Vec<super::generate::PendingArtifact>| {
            let pool = pool.clone();
            let worksheet_id = worksheet_id.clone();
            Box::pin(async move {
                let _ = super::persist_artifacts(&pool, &worksheet_id, &artifacts).await;
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        }) as super::generate::PersistFn
    };

    let (pending, telemetry) = super::generate_all(
        backend,
        concurrency,
        &segments,
        existing,
        None,
        Some(on_persist),
        Some(on_generate),
        Some(logs.clone()),
    )
    .await?;
    println!(
        "[{}] [pipeline] generated {} merged artifacts across {} requests (~{}k in / ~{}k out tokens) in {:?}",
        crate::logging::rfc3339_utc(),
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
                job.artifact_type = tick.artifact_type;
                job.done = tick.done;
                job.total = tick.total;
                job.per_type = tick.per_type;
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
            "types": job.per_type,
            "types_done": job.types_done,
            "types_total": job.types_total,
            "requests_done": job.requests_done,
            "tokens_in": job.tokens_in,
            "tokens_out": job.tokens_out,
            "error": null,
        }),
    );
}
