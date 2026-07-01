#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct Chunk {
    pub id: String,
    pub worksheet_id: String,
    pub file_id: String,
    pub position: i32,
    pub text: String,
    pub embedding: Option<Vec<f32>>,
}

#[derive(Clone)]
pub struct Cluster {
    pub centroid: Vec<f32>,
    pub chunk_ids: Vec<String>,
}

#[derive(Clone, Serialize)]
pub struct Artifact {
    pub id: String,
    pub worksheet_id: String,
    pub artifact_type: ArtifactType,
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

#[derive(Clone, Serialize, Deserialize)]
pub enum ArtifactType {
    MultipleChoiceQuiz,
    EssayQuiz,
    CompletionQuiz,
    Summary,
    MindMap,
}
