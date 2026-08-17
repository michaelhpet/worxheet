#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize)]
pub struct Chunk {
    pub id: String,
    pub worksheet_id: String,
    pub file_id: String,
    pub position: i32,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
}

#[derive(Clone)]
pub struct Cluster {
    pub centroid: Vec<f32>,
    pub chunk_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Artifact {
    pub id: String,
    pub worksheet_id: String,
    pub artifact_type: ArtifactType,
    pub source: String,
    pub content: String,
}

#[derive(Clone, Serialize)]
pub struct Paginated<T: Serialize> {
    pub items: Vec<T>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

/// Live state of the automatic worksheet pipeline, shared with the frontend
/// through `get_pipeline_status` and the `pipeline-progress` event.
#[derive(Clone, Debug, Serialize)]
pub struct PipelineStatus {
    /// One of `idle`, `running`, `done`, `failed`.
    pub status: String,
    /// `ingesting` or `generating` while running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// Artifact type currently being generated (generation phase only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_type: Option<String>,
    /// Units completed in the current phase.
    pub done: usize,
    /// Total units in the current phase.
    pub total: usize,
    /// Artifact types completed so far.
    pub types_done: usize,
    /// Total artifact types to generate.
    pub types_total: usize,
    /// Error message when `status` is `failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ArtifactType {
    MultipleChoiceQuiz,
    EssayQuiz,
    CompletionQuiz,
    Summary,
    MindMap,
}

impl ArtifactType {
    /// The string stored in the `artifacts.artifact_type` column.
    pub fn to_db(&self) -> &'static str {
        match self {
            Self::MultipleChoiceQuiz => "MultipleChoiceQuiz",
            Self::EssayQuiz => "EssayQuiz",
            Self::CompletionQuiz => "CompletionQuiz",
            Self::Summary => "Summary",
            Self::MindMap => "MindMap",
        }
    }

    /// Parse a value read back from the `artifacts.artifact_type` column.
    pub fn from_db(value: &str) -> Result<Self, String> {
        match value {
            "MultipleChoiceQuiz" => Ok(Self::MultipleChoiceQuiz),
            "EssayQuiz" => Ok(Self::EssayQuiz),
            "CompletionQuiz" => Ok(Self::CompletionQuiz),
            "Summary" => Ok(Self::Summary),
            "MindMap" => Ok(Self::MindMap),
            other => Err(format!("Unknown artifact type: {other}")),
        }
    }
}
