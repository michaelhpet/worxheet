use serde::{Deserialize, Serialize};

/// Stored in the `chunks` table (historical name).
#[derive(Clone, Debug, Serialize)]
pub struct Segment {
    pub id: String,
    pub worksheet_id: String,
    pub file_id: String,
    pub position: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
    pub text: String,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PipelineState {
    Idle,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl PipelineState {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "idle" => Some(Self::Idle),
            "running" => Some(Self::Running),
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Ingesting,
    Generating,
}

#[derive(Clone, Debug, Serialize)]
pub struct PipelineStatus {
    pub status: PipelineState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<Phase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_type: Option<ArtifactType>,
    pub done: usize,
    pub total: usize,
    pub types: Vec<TypeProgress>,
    pub types_done: usize,
    pub types_total: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub requests_done: usize,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub tokens_in: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub tokens_out: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TypeProgress {
    pub artifact_type: ArtifactType,
    pub done: usize,
    pub total: usize,
}

impl PipelineStatus {
    pub fn running(
        phase: Phase,
        artifact_type: Option<ArtifactType>,
        done: usize,
        total: usize,
        types: Vec<TypeProgress>,
        types_done: usize,
    ) -> Self {
        Self {
            status: PipelineState::Running,
            phase: Some(phase),
            artifact_type,
            done,
            total,
            types,
            types_done,
            types_total: ArtifactType::ALL.len(),
            requests_done: 0,
            tokens_in: 0,
            tokens_out: 0,
            error: None,
        }
    }

    pub fn terminal(status: PipelineState, error: Option<String>) -> Self {
        Self {
            status,
            phase: None,
            artifact_type: None,
            done: 0,
            total: 0,
            types: Vec::new(),
            types_done: if status == PipelineState::Done {
                ArtifactType::ALL.len()
            } else {
                0
            },
            types_total: ArtifactType::ALL.len(),
            requests_done: 0,
            tokens_in: 0,
            tokens_out: 0,
            error,
        }
    }
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
    pub fn to_db(&self) -> &'static str {
        match self {
            Self::MultipleChoiceQuiz => "MultipleChoiceQuiz",
            Self::EssayQuiz => "EssayQuiz",
            Self::CompletionQuiz => "CompletionQuiz",
            Self::Summary => "Summary",
            Self::MindMap => "MindMap",
        }
    }

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

    pub const QUIZ: [ArtifactType; 3] = [
        ArtifactType::MultipleChoiceQuiz,
        ArtifactType::EssayQuiz,
        ArtifactType::CompletionQuiz,
    ];
}
