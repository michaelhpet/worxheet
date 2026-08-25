#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// One contiguous, ordered generation unit carved out of a worksheet file by
/// the segmenter. Stored in the `chunks` table (historical name).
#[derive(Clone, Debug, Serialize)]
pub struct Segment {
    pub id: String,
    pub worksheet_id: String,
    pub file_id: String,
    /// Global position within the worksheet (document order).
    pub position: i32,
    /// Nearest enclosing heading breadcrumb, if the source had structure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
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
    /// Provider requests issued during this run so far.
    #[serde(skip_serializing_if = "is_zero")]
    pub requests_done: usize,
    /// Approximate prompt tokens observed so far (chars/4 heuristic).
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub tokens_in: u64,
    /// Approximate completion tokens observed so far.
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub tokens_out: u64,
    /// Error message when `status` is `failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
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

    pub const ALL: [ArtifactType; 5] = [
        ArtifactType::MultipleChoiceQuiz,
        ArtifactType::EssayQuiz,
        ArtifactType::CompletionQuiz,
        ArtifactType::Summary,
        ArtifactType::MindMap,
    ];
}
