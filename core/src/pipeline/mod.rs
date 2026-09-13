//! Pipeline orchestration: parse → segment → store → generate → persist.

use sqlx::SqlitePool;
use std::sync::Arc;

use crate::schema::{Artifact, ArtifactType, Segment};

pub mod generate;
pub mod ingest;
pub mod jobs;
pub mod segment;
pub mod validate;

pub use generate::{generate_all, PendingArtifact};
pub use jobs::{remove_job, resume_if_needed, resume_stale, start_job, stop_job, PipelineJobs};

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("Pipeline cancelled by user.")]
    Cancelled,
    #[error("Worksheet has no files to process.")]
    NoFiles,
    #[error("No segments found for this worksheet. Run ingestion first.")]
    NoSegments,
    #[error("No usable segments found for this worksheet. Re-ingest the source files.")]
    NoUsableSegments,
    #[error("{0}")]
    Failed(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

pub type PipelineResult<T> = Result<T, PipelineError>;

#[derive(Clone, Debug)]
pub struct Stop {
    flag: Arc<std::sync::atomic::AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl Stop {
    pub fn new() -> Self {
        Self {
            flag: Arc::new(false.into()),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    #[cfg(test)]
    pub fn never() -> Self {
        Self::new()
    }

    pub fn stop(&self) {
        self.flag.store(true, std::sync::atomic::Ordering::Relaxed);
        self.notify.notify_waiters();
    }

    pub fn is_stopped(&self) -> bool {
        self.flag.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub async fn stopped(&self) {
        // Create the waiter before checking the flag: a stop landing between
        // the two still wakes us; an earlier stop takes the early return.
        let notified = self.notify.notified();
        tokio::pin!(notified);
        if self.is_stopped() {
            return;
        }
        notified.await;
    }

    pub fn check(&self) -> PipelineResult<()> {
        if self.is_stopped() {
            Err(PipelineError::Cancelled)
        } else {
            Ok(())
        }
    }
}

pub async fn process_files(
    pool: &SqlitePool,
    worksheet_id: &str,
    file_ids: &[String],
    start_position: i32,
    tokenizer: Arc<tokenizers::Tokenizer>,
    logs: Option<Arc<crate::logging::RunLogs>>,
    stop: Stop,
) -> PipelineResult<Vec<Segment>> {
    ingest::process_files(
        pool,
        worksheet_id,
        file_ids,
        start_position,
        tokenizer,
        logs,
        stop,
    )
    .await
}

pub async fn unchunked_files(
    pool: &SqlitePool,
    worksheet_id: &str,
    file_ids: &[String],
) -> PipelineResult<Vec<String>> {
    ingest::unchunked_files(pool, worksheet_id, file_ids).await
}

pub async fn next_segment_position(pool: &SqlitePool, worksheet_id: &str) -> PipelineResult<i32> {
    ingest::next_segment_position(pool, worksheet_id).await
}

pub async fn reuse_chunks(
    pool: &SqlitePool,
    worksheet_id: &str,
    file_ids: &[String],
    start_position: i32,
) -> PipelineResult<(Vec<String>, i32)> {
    ingest::reuse_chunks(pool, worksheet_id, file_ids, start_position).await
}

pub async fn load_segments(pool: &SqlitePool, worksheet_id: &str) -> PipelineResult<Vec<Segment>> {
    let rows = sqlx::query_as::<_, (String, String, i32, Option<String>, String)>(
        "SELECT id, file_id, position, heading, text
         FROM chunks
         WHERE worksheet_id = ?
         ORDER BY position",
    )
    .bind(worksheet_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, file_id, position, heading, text)| Segment {
            id,
            worksheet_id: worksheet_id.to_string(),
            file_id,
            position,
            heading,
            text,
        })
        .collect())
}

pub async fn load_existing_artifacts(
    pool: &SqlitePool,
    worksheet_id: &str,
) -> PipelineResult<generate::ExistingArtifacts> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT artifact_type, source FROM artifacts WHERE worksheet_id = ?")
            .bind(worksheet_id)
            .fetch_all(pool)
            .await?;

    let mut existing = generate::ExistingArtifacts::default();
    for (artifact_type, source) in rows {
        match ArtifactType::from_db(&artifact_type) {
            Ok(ok_type) if items_field_is_merged(&ok_type) => {
                existing.done_merged.insert(artifact_type);
            }
            Ok(_) => {
                existing.done_items.insert((artifact_type, source));
            }
            Err(_) => {}
        }
    }
    Ok(existing)
}

fn items_field_is_merged(artifact_type: &ArtifactType) -> bool {
    matches!(artifact_type, ArtifactType::Summary | ArtifactType::MindMap)
}

pub async fn persist_artifacts(
    pool: &SqlitePool,
    worksheet_id: &str,
    pending: &[PendingArtifact],
) -> PipelineResult<Vec<Artifact>> {
    let mut transaction = pool.begin().await?;

    let mut artifacts = Vec::with_capacity(pending.len());
    for item in pending {
        let artifact = Artifact {
            id: ulid::Ulid::new().to_string(),
            worksheet_id: worksheet_id.to_string(),
            artifact_type: item.artifact_type.clone(),
            source: item.source.clone(),
            content: item.content.clone(),
        };
        sqlx::query(
            "INSERT INTO artifacts (id, worksheet_id, artifact_type, source, content)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&artifact.id)
        .bind(worksheet_id)
        .bind(artifact.artifact_type.to_db())
        .bind(&artifact.source)
        .bind(&artifact.content)
        .execute(&mut *transaction)
        .await?;
        artifacts.push(artifact);
    }

    transaction.commit().await?;

    Ok(artifacts)
}

pub async fn get_artifacts(
    pool: &SqlitePool,
    worksheet_id: &str,
    artifact_type: &ArtifactType,
    count: Option<i64>,
) -> PipelineResult<Vec<Artifact>> {
    let limit = count.map(|c| c.max(1)).unwrap_or(-1);
    let rows = sqlx::query_as::<_, (String, String, String, String, String)>(
        "SELECT id, worksheet_id, artifact_type, source, content
         FROM artifacts
         WHERE worksheet_id = ? AND artifact_type = ?
         ORDER BY RANDOM()
         LIMIT ?",
    )
    .bind(worksheet_id)
    .bind(artifact_type.to_db())
    .bind(limit)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|(id, worksheet_id, artifact_type, source, content)| {
            Ok(Artifact {
                id,
                worksheet_id,
                artifact_type: ArtifactType::from_db(&artifact_type)
                    .map_err(PipelineError::Failed)?,
                source,
                content,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::generate::{ExistingArtifacts, GenerationParams};
    use crate::provider::mock::MockBackend;
    use crate::provider::{ArtifactBackend, ProviderError};
    use std::sync::Arc;

    #[allow(unused_imports)]
    use generate as _generate_alias;
    use serde_json::json;
    use sqlx::SqlitePool;

    async fn setup_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("in-memory pool");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("migrations");
        pool
    }

    async fn seed_worksheet(pool: &SqlitePool) -> String {
        let id = ulid::Ulid::new().to_string();
        sqlx::query("INSERT INTO worksheets (id, name) VALUES (?, ?)")
            .bind(&id)
            .bind("test")
            .execute(pool)
            .await
            .unwrap();
        id
    }

    async fn seed_artifact(pool: &SqlitePool, worksheet_id: &str, artifact_type: &str) {
        sqlx::query(
            "INSERT INTO artifacts (id, worksheet_id, artifact_type, source, content)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(ulid::Ulid::new().to_string())
        .bind(worksheet_id)
        .bind(artifact_type)
        .bind("src")
        .bind("{}")
        .execute(pool)
        .await
        .unwrap();
    }

    fn test_segments(count: usize) -> Vec<Segment> {
        (0..count)
            .map(|index| Segment {
                id: format!("seg-{index}"),
                worksheet_id: String::from("ws"),
                file_id: String::from("file"),
                position: index as i32,
                heading: None,
                text: if index % 2 == 0 {
                    String::from(
                        "The mitochondrion is the powerhouse of the cell where respiration \
                         produces ATP through glycolysis and the citric acid cycle.",
                    )
                } else {
                    String::from(
                        "Volcanic eruptions occur when magma pressure builds beneath the crust, \
                         releasing ash and lava onto the surrounding landscape.",
                    )
                },
            })
            .collect()
    }

    #[tokio::test]
    async fn test_get_artifacts_filters_by_type_and_count() {
        let pool = setup_db().await;
        let worksheet_id = seed_worksheet(&pool).await;
        for _ in 0..5 {
            seed_artifact(&pool, &worksheet_id, "MultipleChoiceQuiz").await;
        }
        for _ in 0..3 {
            seed_artifact(&pool, &worksheet_id, "EssayQuiz").await;
        }

        let mcqs = get_artifacts(
            &pool,
            &worksheet_id,
            &ArtifactType::MultipleChoiceQuiz,
            None,
        )
        .await
        .unwrap();
        assert_eq!(mcqs.len(), 5);

        let limited = get_artifacts(
            &pool,
            &worksheet_id,
            &ArtifactType::MultipleChoiceQuiz,
            Some(2),
        )
        .await
        .unwrap();
        assert_eq!(limited.len(), 2);

        let clamped = get_artifacts(&pool, &worksheet_id, &ArtifactType::EssayQuiz, Some(0))
            .await
            .unwrap();
        assert_eq!(clamped.len(), 1);
    }

    #[tokio::test]
    async fn test_generate_all_produces_and_validates_with_mock_backend() {
        let backend = Arc::new(MockBackend::with_responder(move |request| {
            let source_has_mitochondria = request.user.contains("mitochondrion");
            match request.schema_name.as_str() {
                "MultipleChoiceQuiz" => Ok(if source_has_mitochondria {
                    json!({
                        "questions": [{
                            "question": "Where does respiration produce ATP?",
                            "options": ["The mitochondrion", "The nucleus", "Ribosomes", "Vacuoles"],
                            "answer": "The mitochondrion",
                            "explanation": "Respiration happens there."
                        }]
                    })
                } else {
                    json!({
                        "questions": [{
                            "question": "What builds beneath the crust before an eruption?",
                            "options": ["Magma pressure", "Ocean tides", "Wind shear", "Sediment"],
                            "answer": "Magma pressure",
                            "explanation": "Pressure accumulates underground."
                        }]
                    })
                }
                .to_string()),
                "EssayQuiz" => Ok(json!({
                    "questions": [{
                        "question": if source_has_mitochondria {
                            "Explain how the citric acid cycle supports ATP production."
                        } else {
                            "Describe how magma pressure leads to volcanic eruptions."
                        },
                        "instructions": "Ground every claim in the material.",
                        "model_answer": "Summarize the causal chain stated in the material."
                    }]
                })
                .to_string()),
                "CompletionQuiz" => Ok(json!({
                    "items": [{
                        "sentence": if source_has_mitochondria {
                            "The ____________ is the powerhouse of the cell."
                        } else {
                            "Eruptions release ash and ____________ onto the landscape."
                        },
                        "answer": if source_has_mitochondria { "mitochondrion" } else { "lava" },
                        "hint": "from the material"
                    }]
                })
                .to_string()),
                "Summary" => Ok(json!({
                    "title": if source_has_mitochondria { "Cell Biology" } else { "Geology" },
                    "summary": if source_has_mitochondria {
                        "Respiration happens in mitochondria and produces ATP."
                    } else {
                        "Magma pressure builds until eruptions release ash and lava."
                    },
                    "key_points": ["Grounded key point"]
                })
                .to_string()),
                "MindMap" => Ok(json!({
                    "topic": if source_has_mitochondria { "Cell respiration" } else { "Volcanoes" },
                    "branches": [{ "label": "Stages", "children": ["One", "Two"] }]
                })
                .to_string()),
                other => Err(ProviderError::Rejected(format!("unknown schema {other}"))),
            }
        })) as Arc<dyn ArtifactBackend>;

        let segments = test_segments(2);
        let (pending, telemetry) = generate_all(
            backend.clone(),
            4,
            &segments,
            ExistingArtifacts::default(),
            Some(GenerationParams::default()),
            None,
            None,
            None,
            Stop::never(),
        )
        .await
        .expect("generation should succeed");

        assert_eq!(telemetry.requests, 10);

        for artifact in &pending {
            if matches!(artifact.artifact_type, ArtifactType::MultipleChoiceQuiz) {
                let value: serde_json::Value = serde_json::from_str(&artifact.content).unwrap();
                let options: Vec<&str> = value["options"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect();
                assert!(options.contains(&value["answer"].as_str().unwrap()));
            }
        }

        let count_of = |kind: &str| {
            pending
                .iter()
                .filter(|a| a.artifact_type.to_db() == kind)
                .count()
        };
        assert_eq!(count_of("MultipleChoiceQuiz"), 2);
        assert_eq!(count_of("EssayQuiz"), 2);
        assert_eq!(count_of("CompletionQuiz"), 2);
        assert_eq!(count_of("Summary"), 1);
        assert_eq!(count_of("MindMap"), 1);

        let summary: serde_json::Value = serde_json::from_str(
            &pending
                .iter()
                .find(|a| a.artifact_type.to_db() == "Summary")
                .unwrap()
                .content,
        )
        .unwrap();
        assert!(summary["summary"]
            .as_str()
            .unwrap()
            .contains("mitochondria"));
    }

    #[tokio::test]
    async fn test_generate_all_persists_via_helper() {
        let backend = Arc::new(MockBackend::with_responder(|request| {
            Ok(match request.schema_name.as_str() {
                "Summary" => json!({
                    "title": "T",
                    "summary": "Mitochondria produce ATP through respiration.",
                    "key_points": ["Mitochondria produce ATP"]
                }),
                _ => json!({ "placeholder": true }),
            }
            .to_string())
        })) as Arc<dyn ArtifactBackend>;
        let segments = test_segments(1);
        let (pending, _) = generate_all(
            backend,
            2,
            &segments,
            ExistingArtifacts::default(),
            None,
            None,
            None,
            None,
            Stop::never(),
        )
        .await
        .unwrap();
        assert_eq!(
            pending
                .iter()
                .filter(|a| matches!(a.artifact_type, ArtifactType::Summary))
                .count(),
            1
        );

        let pool = setup_db().await;
        let worksheet_id = seed_worksheet(&pool).await;
        let artifacts = persist_artifacts(&pool, &worksheet_id, &pending)
            .await
            .unwrap();
        assert_eq!(artifacts.len(), pending.len());

        let stored = get_artifacts(&pool, &worksheet_id, &ArtifactType::Summary, None)
            .await
            .unwrap();
        assert_eq!(stored.len(), 1);
    }

    #[tokio::test]
    async fn test_generate_all_stopped_before_start_cancels() {
        let mock = Arc::new(MockBackend::new(vec![]));
        let backend: Arc<dyn ArtifactBackend> = mock.clone();
        let stop = Stop::new();
        stop.stop();
        let result = generate_all(
            backend,
            2,
            &test_segments(1),
            ExistingArtifacts::default(),
            None,
            None,
            None,
            None,
            stop,
        )
        .await;
        assert!(matches!(result, Err(PipelineError::Cancelled)));
        assert_eq!(mock.request_count(), 0, "no units may spawn once stopped");
    }

    #[tokio::test]
    async fn test_load_existing_artifacts_classifies_items_and_merged() {
        let pool = setup_db().await;
        let worksheet_id = seed_worksheet(&pool).await;

        sqlx::query(
            "INSERT INTO artifacts (id, worksheet_id, artifact_type, source, content)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(ulid::Ulid::new().to_string())
        .bind(&worksheet_id)
        .bind("MultipleChoiceQuiz")
        .bind("seg-0")
        .bind("{}")
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO artifacts (id, worksheet_id, artifact_type, source, content)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(ulid::Ulid::new().to_string())
        .bind(&worksheet_id)
        .bind("MultipleChoiceQuiz")
        .bind("seg-1")
        .bind("{}")
        .execute(&pool)
        .await
        .unwrap();
        seed_artifact(&pool, &worksheet_id, "Summary").await;

        let existing = load_existing_artifacts(&pool, &worksheet_id).await.unwrap();

        assert!(existing
            .done_items
            .contains(&("MultipleChoiceQuiz".to_string(), "seg-0".to_string())));
        assert!(existing
            .done_items
            .contains(&("MultipleChoiceQuiz".to_string(), "seg-1".to_string())));
        assert!(!existing
            .done_items
            .contains(&("MultipleChoiceQuiz".to_string(), "seg-2".to_string())));

        assert!(existing.done_merged.contains("Summary"));
        assert!(!existing.done_merged.contains("MindMap"));
        assert_eq!(existing.done_items.len(), 2);
        assert_eq!(existing.done_merged.len(), 1);
    }
}
