//! LLM generation backends. Generation is cloud/server-hosted through an
//! OpenAI-compatible chat-completions surface (OpenAI, Google Gemini's
//! compatibility endpoint, Ollama, LM Studio, any custom deployment); the
//! [`ArtifactBackend`] trait is the single seam every caller depends on.

pub mod client;
pub mod config;
pub mod mock;

use std::fmt;

use async_trait::async_trait;

/// One structured-generation call. `schema` is a JSON Schema document the
/// output must satisfy; providers that support structured outputs enforce it
/// server-side, everything else relies on prompt discipline plus local
/// validation.
#[derive(Clone, Debug)]
pub struct GenerateRequest {
    pub system: String,
    pub user: String,
    pub schema_name: String,
    pub schema: serde_json::Value,
    pub temperature: f32,
    pub max_tokens: i32,
    pub seed: u64,
}

#[derive(Debug)]
#[allow(dead_code)] // reserved for settings gating paths
pub enum ProviderError {
    /// No provider is configured yet (missing key/model).
    Unconfigured,
    /// The provider asked us to slow down.
    RateLimited {
        retry_after: Option<std::time::Duration>,
        message: String,
    },
    /// Connectivity problem; safe to retry.
    Network(String),
    /// Provider-side failure (5xx); safe to retry.
    Server(String),
    /// The request was rejected (4xx); not retryable.
    Rejected(String),
    /// The provider answered but the payload was unusable.
    InvalidResponse(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unconfigured => write!(f, "No LLM provider is configured"),
            Self::RateLimited {
                retry_after,
                message,
            } => match retry_after {
                Some(delay) => write!(
                    f,
                    "Rate limited (retry after {:.0}s): {message}",
                    delay.as_secs_f32()
                ),
                None => write!(f, "Rate limited: {message}"),
            },
            Self::Network(message) => write!(f, "Network error: {message}"),
            Self::Server(message) => write!(f, "Provider server error: {message}"),
            Self::Rejected(message) => write!(f, "Request rejected: {message}"),
            Self::InvalidResponse(message) => write!(f, "Invalid provider response: {message}"),
        }
    }
}

/// Anything that can turn prompts into schema-constrained JSON.
#[async_trait]
pub trait ArtifactBackend: Send + Sync {
    async fn generate_json(&self, request: &GenerateRequest) -> Result<GenerateReply, ProviderError>;

    /// Best-effort listing of model ids for settings UIs.
    async fn list_models(&self) -> Result<Vec<String>, ProviderError>;
}

/// One completed provider reply. `text` is the usable payload (the message
/// `content`, falling back to reasoning fields for thinking models whose
/// content comes back empty); the rest is diagnosis for response logs.
#[derive(Clone, Debug)]
pub struct GenerateReply {
    pub text: String,
    /// Provider-reported stop reason (`stop`, `length`, `content_filter`, ...).
    pub finish_reason: String,
    pub refusal: Option<String>,
    /// Where `text` was read from: `content`, `reasoning`, or `reasoning_content`.
    pub field_source: &'static str,
}
