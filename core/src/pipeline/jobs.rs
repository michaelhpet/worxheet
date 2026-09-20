//! Background pipeline job runner: one task per worksheet, provider resolved
//! per run so settings changes apply immediately.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sqlx::{Pool, Sqlite};
use tauri::{AppHandle, Emitter};

use super::{PipelineError, PipelineResult, Stop};
use crate::provider::{self, client::OpenAiClient, ArtifactBackend};
use crate::schema::{ArtifactType, Phase, PipelineState, PipelineStatus, TypeProgress};
use crate::settings::SettingsState;

use super::segment;

#[derive(Clone)]
pub(crate) struct RunningJob {
    pub(crate) phase: Phase,
    pub(crate) artifact_type: Option<ArtifactType>,
    pub(crate) done: usize,
    pub(crate) total: usize,
    pub(crate) per_type: Vec<TypeProgress>,
    pub(crate) types_done: usize,
    pub(crate) stop: Stop,
}

/// In-memory running pipelines keyed by worksheet id.
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

struct RunCtx {
    app: AppHandle,
    pool: Pool<Sqlite>,
    settings: Arc<SettingsState>,
    jobs: Arc<PipelineJobs>,
    worksheet_id: String,
    stop: Stop,
    only_types: Option<Vec<ArtifactType>>,
}

impl RunCtx {
    fn check_cancelled(&self) -> PipelineResult<()> {
        self.stop.check()
    }

    fn update_progress(&self, update: impl FnOnce(&mut RunningJob)) {
        apply_progress(&self.app, &self.jobs, &self.worksheet_id, update);
    }

    fn set_phase(&self, phase: Phase, artifact_type: Option<ArtifactType>) {
        self.update_progress(|job| {
            job.phase = phase;
            job.artifact_type = artifact_type;
            job.done = 0;
            job.total = 0;
        });
    }

    fn generate_sink(&self) -> super::generate::ProgressFn {
        let (app, jobs, worksheet_id) = self.sink_parts();
        Arc::new(move |tick| {
            apply_progress(&app, &jobs, &worksheet_id, |job| {
                job.artifact_type = tick.artifact_type;
                job.done = tick.done;
                job.total = tick.total;
                job.per_type = tick.per_type;
                job.types_done = tick.types_done;
            });
        })
    }

    fn sink_parts(&self) -> (AppHandle, Arc<PipelineJobs>, String) {
        (
            self.app.clone(),
            self.jobs.clone(),
            self.worksheet_id.clone(),
        )
    }
}

fn apply_progress(
    app: &AppHandle,
    jobs: &Arc<PipelineJobs>,
    worksheet_id: &str,
    update: impl FnOnce(&mut RunningJob),
) {
    {
        let mut running = jobs.jobs.lock().unwrap();
        if let Some(job) = running.get_mut(worksheet_id) {
            update(job);
        }
    }
    emit_progress(app, jobs, worksheet_id);
}

/// Returns `false` without doing anything if the worksheet already has a live job.
pub fn start_job(
    app: AppHandle,
    pool: Pool<Sqlite>,
    settings: Arc<SettingsState>,
    jobs: Arc<PipelineJobs>,
    worksheet_id: String,
) -> bool {
    start_job_for_types(app, pool, settings, jobs, worksheet_id, None)
}

/// Same as [`start_job`] but restricts generation to `only_types`.
/// Ingestion still processes pending files; only the generation fan-out is
/// filtered. Returns `false` if the worksheet already has a live job.
pub fn start_job_for_types(
    app: AppHandle,
    pool: Pool<Sqlite>,
    settings: Arc<SettingsState>,
    jobs: Arc<PipelineJobs>,
    worksheet_id: String,
    only_types: Option<Vec<ArtifactType>>,
) -> bool {
    let ctx = RunCtx {
        app,
        pool,
        settings,
        jobs,
        worksheet_id,
        stop: Stop::new(),
        only_types,
    };
    {
        let mut running = ctx.jobs.jobs.lock().unwrap();
        if running.contains_key(&ctx.worksheet_id) {
            return false;
        }
        let stop = ctx.stop.clone();
        running.insert(
            ctx.worksheet_id.clone(),
            RunningJob {
                phase: Phase::Ingesting,
                artifact_type: None,
                done: 0,
                total: 0,
                per_type: Vec::new(),
                types_done: 0,
                stop,
            },
        );
    }

    tauri::async_runtime::spawn(async move {
        run_job(ctx).await;
    });
    true
}

/// Whether the worksheet currently has a live job.
pub fn is_running(jobs: &Arc<PipelineJobs>, worksheet_id: &str) -> bool {
    jobs.jobs.lock().unwrap().contains_key(worksheet_id)
}

/// Idempotent: returns `false` when nothing was running. The task exits without
/// overwriting the `cancelled` row persisted here.
pub async fn stop_job(
    app: &AppHandle,
    pool: &Pool<Sqlite>,
    jobs: &Arc<PipelineJobs>,
    worksheet_id: &str,
) -> bool {
    let live = jobs
        .jobs
        .lock()
        .unwrap()
        .get(worksheet_id)
        .map(|job| job.stop.clone());

    if let Some(stop) = live {
        stop.stop();
        // Drop the snapshot so polls see `cancelled` at once; the task holds its own Stop.
        jobs.jobs.lock().unwrap().remove(worksheet_id);
        // Report stopped even if the row already flipped: `run_job` lets a
        // cancelled outcome win over a racing completion.
        if let Err(error) = transition_status(
            pool,
            worksheet_id,
            PipelineState::Cancelled,
            None,
            &[PipelineState::Running, PipelineState::Idle],
        )
        .await
        {
            eprintln!("[pipeline] failed to persist cancel status: {error}");
        }
        emit_status(
            app,
            worksheet_id,
            &PipelineStatus::terminal(PipelineState::Cancelled, None),
        );
        return true;
    }

    let flipped = transition_status(
        pool,
        worksheet_id,
        PipelineState::Cancelled,
        None,
        &[PipelineState::Running, PipelineState::Idle],
    )
    .await
    .unwrap_or(false);
    if flipped {
        emit_status(
            app,
            worksheet_id,
            &PipelineStatus::terminal(PipelineState::Cancelled, None),
        );
    }
    flipped
}

pub async fn resume_stale(
    app: AppHandle,
    pool: Pool<Sqlite>,
    settings: Arc<SettingsState>,
    jobs: Arc<PipelineJobs>,
) {
    let rows =
        sqlx::query_as::<_, (String,)>("SELECT id FROM worksheets WHERE pipeline_status = ?")
            .bind(PipelineState::Running.as_db_str())
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

/// Signals cancellation first so the orphaned task stops instead of
/// continuing provider requests for a deleted row.
pub fn remove_job(jobs: &Arc<PipelineJobs>, worksheet_id: &str) {
    let mut running = jobs.jobs.lock().unwrap();
    if let Some(job) = running.get(worksheet_id) {
        job.stop.stop();
    }
    running.remove(worksheet_id);
}

pub async fn get_status(
    pool: &Pool<Sqlite>,
    jobs: &Arc<PipelineJobs>,
    worksheet_id: &str,
) -> PipelineResult<PipelineStatus> {
    if let Some(running) = jobs.jobs.lock().unwrap().get(worksheet_id) {
        return Ok(PipelineStatus::running(
            running.phase,
            running.artifact_type.clone(),
            running.done,
            running.total,
            running.per_type.clone(),
            running.types_done,
        ));
    }

    let (persisted, persisted_error): (String, Option<String>) =
        sqlx::query_as("SELECT pipeline_status, pipeline_error FROM worksheets WHERE id = ?")
            .bind(worksheet_id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| PipelineError::Failed(String::from("Worksheet not found")))?;

    let state = PipelineState::from_db_str(&persisted).unwrap_or(PipelineState::Failed);
    Ok(PipelineStatus::terminal(state, persisted_error))
}

async fn persist_status(
    pool: &Pool<Sqlite>,
    worksheet_id: &str,
    state: PipelineState,
    error: Option<&str>,
) -> PipelineResult<()> {
    transition_status(pool, worksheet_id, state, error, &[]).await?;
    Ok(())
}

/// Unknown stored values never match, so conditional writes simply don't fire.
async fn transition_status(
    pool: &Pool<Sqlite>,
    worksheet_id: &str,
    state: PipelineState,
    error: Option<&str>,
    from: &[PipelineState],
) -> PipelineResult<bool> {
    let mut builder = sqlx::QueryBuilder::new("UPDATE worksheets SET pipeline_status = ");
    builder.push_bind(state.as_db_str());
    builder.push(", pipeline_error = ");
    builder.push_bind(error);
    builder.push(" WHERE id = ");
    builder.push_bind(worksheet_id);
    if !from.is_empty() {
        builder.push(" AND pipeline_status IN (");
        let mut separated = builder.separated(", ");
        for state in from {
            separated.push_bind(state.as_db_str());
        }
        separated.push_unseparated(")");
    }
    Ok(builder.build().execute(pool).await?.rows_affected() > 0)
}

/// Safe to call on every status poll.
pub async fn resume_if_needed(
    app: AppHandle,
    pool: &Pool<Sqlite>,
    settings: &Arc<SettingsState>,
    jobs: &Arc<PipelineJobs>,
    worksheet_id: &str,
) -> PipelineResult<PipelineStatus> {
    let live = jobs.jobs.lock().unwrap().contains_key(worksheet_id);
    if live {
        return get_status(pool, jobs, worksheet_id).await;
    }

    let persisted: Option<(String,)> =
        sqlx::query_as("SELECT pipeline_status FROM worksheets WHERE id = ?")
            .bind(worksheet_id)
            .fetch_optional(pool)
            .await?;

    let Some((status,)) = persisted else {
        return Err(PipelineError::Failed(String::from("Worksheet not found")));
    };

    // Terminal states and unknown values surface as-is for a deliberate retry.
    match PipelineState::from_db_str(&status) {
        Some(state) if !state.is_terminal() => {
            start_job(
                app,
                pool.clone(),
                settings.clone(),
                jobs.clone(),
                worksheet_id.to_string(),
            );
            get_status(pool, jobs, worksheet_id).await
        }
        _ => get_status(pool, jobs, worksheet_id).await,
    }
}

async fn run_job(ctx: RunCtx) {
    let result = run_pipeline(&ctx).await;
    let worksheet_id = ctx.worksheet_id.clone();

    // A stop request wins over a racing completion.
    if ctx.stop.is_stopped() || matches!(&result, Err(PipelineError::Cancelled)) {
        if let Err(error) = transition_status(
            &ctx.pool,
            &worksheet_id,
            PipelineState::Cancelled,
            None,
            &[PipelineState::Running],
        )
        .await
        {
            eprintln!("[pipeline] failed to persist cancel status: {error}");
        }
        finish_job(
            &ctx,
            &PipelineStatus::terminal(PipelineState::Cancelled, None),
        );
        return;
    }

    let (status, error) = match result {
        Ok(()) => (PipelineState::Done, None),
        Err(error) => {
            eprintln!("[pipeline] worksheet {worksheet_id} failed: {error}");
            (PipelineState::Failed, Some(error.to_string()))
        }
    };

    if let Err(error) = persist_status(&ctx.pool, &worksheet_id, status, error.as_deref()).await {
        eprintln!("[pipeline] failed to persist terminal status: {error}");
    }
    finish_job(&ctx, &PipelineStatus::terminal(status, error));
}

fn finish_job(ctx: &RunCtx, status: &PipelineStatus) {
    ctx.jobs.jobs.lock().unwrap().remove(&ctx.worksheet_id);
    emit_status(&ctx.app, &ctx.worksheet_id, status);
}

fn emit_status(app: &AppHandle, worksheet_id: &str, status: &PipelineStatus) {
    let mut value = serde_json::to_value(status).unwrap_or(serde_json::Value::Null);
    value["worksheet_id"] = worksheet_id.into();
    let _ = app.emit("pipeline-progress", value);
}

fn generation_params(settings: &Arc<SettingsState>) -> super::generate::GenerationParams {
    let artifacts = &settings.get().artifacts;
    super::generate::GenerationParams {
        temperature: artifacts.temperature,
        max_tokens: artifacts.max_tokens.min(i32::MAX as u32) as i32,
        seed: match artifacts.seed {
            Some(seed) => seed as u64,
            None => std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0),
        },
    }
}

fn resolve_backend(
    settings: &Arc<SettingsState>,
) -> PipelineResult<(Arc<dyn ArtifactBackend>, usize)> {
    let provider = settings.get().provider;
    let config = provider.active_config();
    if !config.is_configured() {
        return Err(PipelineError::Failed(String::from(
            "No inference provider is configured. Open Settings and add a provider.",
        )));
    }
    let api_key = provider::config::active_key(&provider.active).map_err(PipelineError::Failed)?;
    let backend = OpenAiClient::new(&config.base_url, api_key, &config.model)
        .with_reasoning_effort(config.disable_thinking.then(|| String::from("none")));
    Ok((Arc::new(backend), config.concurrency))
}

async fn run_pipeline(ctx: &RunCtx) -> PipelineResult<()> {
    ctx.check_cancelled()?;
    persist_status(&ctx.pool, &ctx.worksheet_id, PipelineState::Running, None)
        .await
        .map_err(|_| PipelineError::Failed(String::from("Failed to mark worksheet as running")))?;

    let file_ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM files WHERE worksheet_id = ? ORDER BY created_at")
            .bind(&ctx.worksheet_id)
            .fetch_all(&ctx.pool)
            .await
            .map_err(|e| PipelineError::Failed(format!("Failed to load files: {e}")))?;

    if file_ids.is_empty() {
        return Err(PipelineError::NoFiles);
    }

    let pending_files = super::unchunked_files(&ctx.pool, &ctx.worksheet_id, &file_ids).await?;

    // Fail fast on missing provider config before doing any local work.
    let (backend, concurrency) = resolve_backend(&ctx.settings)?;
    let generation = generation_params(&ctx.settings);
    let tokenizer = segment::bundled_tokenizer()?;
    let logs = Arc::new(crate::logging::RunLogs::new());

    let start_position = super::next_segment_position(&ctx.pool, &ctx.worksheet_id).await?;
    let (to_parse, start_position) =
        super::reuse_chunks(&ctx.pool, &ctx.worksheet_id, &pending_files, start_position).await?;

    ctx.check_cancelled()?;
    let tokenizer = Arc::new(tokenizer);
    super::process_files(
        &ctx.pool,
        &ctx.worksheet_id,
        &to_parse,
        start_position,
        tokenizer,
        Some(logs.clone()),
        ctx.stop.clone(),
    )
    .await?;
    ctx.check_cancelled()?;

    ctx.set_phase(Phase::Generating, None);

    let segments = super::load_segments(&ctx.pool, &ctx.worksheet_id).await?;
    let existing = super::load_existing_artifacts(&ctx.pool, &ctx.worksheet_id).await?;
    let started = std::time::Instant::now();

    // Per-item artifacts persist as each unit completes so an interruption
    // keeps finished parts; merged types persist once below. Persist failures
    // fall back to the final persist instead of silently dropping data.
    let on_persist = {
        let pool = ctx.pool.clone();
        let worksheet_id = ctx.worksheet_id.clone();
        Arc::new(move |artifacts: Vec<super::generate::PendingArtifact>| {
            let pool = pool.clone();
            let worksheet_id = worksheet_id.clone();
            Box::pin(async move {
                super::persist_artifacts(&pool, &worksheet_id, &artifacts).await?;
                Ok(())
            })
                as std::pin::Pin<
                    Box<dyn std::future::Future<Output = super::PipelineResult<()>> + Send>,
                >
        }) as super::generate::PersistFn
    };

    let (pending, telemetry) = super::generate_all(
        backend,
        concurrency,
        &segments,
        existing,
        Some(generation),
        Some(on_persist),
        Some(ctx.generate_sink()),
        Some(logs.clone()),
        ctx.stop.clone(),
        ctx.only_types.as_deref(),
    )
    .await?;
    ctx.check_cancelled()?;
    println!(
        "[{}] [pipeline] generated {} merged artifacts across {} requests (~{}k in / ~{}k out tokens) in {:?}",
        crate::logging::rfc3339_utc(),
        pending.len(),
        telemetry.requests,
        telemetry.tokens_in / 1000,
        telemetry.tokens_out / 1000,
        started.elapsed()
    );

    super::persist_artifacts(&ctx.pool, &ctx.worksheet_id, &pending).await?;

    Ok(())
}

fn emit_progress(app: &AppHandle, jobs: &Arc<PipelineJobs>, worksheet_id: &str) {
    let snapshot = jobs.jobs.lock().unwrap().get(worksheet_id).cloned();
    let Some(job) = snapshot else {
        return;
    };
    emit_status(
        app,
        worksheet_id,
        &PipelineStatus::running(
            job.phase,
            job.artifact_type,
            job.done,
            job.total,
            job.per_type,
            job.types_done,
        ),
    );
}
