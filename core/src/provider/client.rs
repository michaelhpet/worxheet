//! OpenAI-compatible chat-completions client with retry/backoff and a
//! graceful fallback when a server rejects structured outputs.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use super::{ArtifactBackend, GenerateRequest, ProviderError};

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);
const MAX_ATTEMPTS: usize = 4;

fn backoff_delay(attempt: usize) -> std::time::Duration {
    // 700ms, 1.6s, 3.2s …
    std::time::Duration::from_millis(700 * (1 << attempt.min(3)) / 2 + 200)
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

/// Client for any OpenAI-compatible `/chat/completions` deployment.
pub struct OpenAiClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    model: String,
}

impl OpenAiClient {
    pub fn new(base_url: &str, api_key: Option<String>, model: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .unwrap_or_default(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model: model.to_string(),
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path)
    }

    fn auth_headers(&self, mut request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        request
    }
}

#[async_trait]
impl ArtifactBackend for OpenAiClient {
    async fn generate_json(&self, request: &GenerateRequest) -> Result<String, ProviderError> {
        let mut body = json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": request.system },
                { "role": "user", "content": request.user },
            ],
            "temperature": request.temperature,
            "max_tokens": request.max_tokens,
            "seed": request.seed,
        });
        if !request.schema.is_null() {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {
                    "name": request.schema_name,
                    "strict": true,
                    "schema": request.schema,
                },
            });
        }

        let mut structured = !request.schema.is_null();
        let mut last_error = ProviderError::InvalidResponse(String::from("no attempts made"));

        for attempt in 0..MAX_ATTEMPTS {
            let payload = if structured {
                body.clone()
            } else {
                let mut stripped = body.clone();
                stripped.as_object_mut().map(|map| map.remove("response_format"));
                stripped
            };

            let response = self
                .auth_headers(self.http.post(self.endpoint("chat/completions")))
                .json(&payload)
                .send()
                .await;

            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    last_error = ProviderError::Network(error.to_string());
                    sleep_backoff(attempt).await;
                    continue;
                }
            };

            let status = response.status();
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_after = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<u64>().ok())
                    .map(std::time::Duration::from_secs);
                let message = response.text().await.unwrap_or_default();
                last_error = ProviderError::RateLimited { retry_after, message };
                match retry_after {
                    Some(delay) => tokio::time::sleep(delay).await,
                    None => sleep_backoff(attempt).await,
                }
                continue;
            }

            if status.is_server_error() {
                let message = response.text().await.unwrap_or_default();
                last_error = ProviderError::Server(format!("{status}: {message}"));
                sleep_backoff(attempt).await;
                continue;
            }

            if status.is_client_error() {
                let text = response.text().await.unwrap_or_default();
                // Older deployments reject `response_format` outright; fall
                // back to prompt-only JSON once before giving up.
                if structured && mentions_unsupported_schema(&text) {
                    structured = false;
                    continue;
                }
                return Err(ProviderError::Rejected(format!("{status}: {text}")));
            }

            let completion: ChatCompletionResponse = response.json().await.map_err(|error| {
                ProviderError::InvalidResponse(format!("failed to decode completion: {error}"))
            })?;

            let content = completion
                .choices
                .first()
                .and_then(|choice| choice.message.content.clone())
                .ok_or_else(|| ProviderError::InvalidResponse(String::from("completion had no message content")))?;

            return Ok(strip_code_fence(&content));
        }

        Err(last_error)
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        let response = self
            .auth_headers(self.http.get(self.endpoint("models")))
            .send()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ProviderError::RateLimited {
                retry_after: None,
                message: String::from("model list rate limited"),
            });
        }
        if status.is_client_error() {
            return Err(ProviderError::Rejected(format!("model listing failed ({status})")));
        }
        if status.is_server_error() {
            return Err(ProviderError::Server(format!("model listing failed ({status})")));
        }

        let models: ModelsResponse = response
            .json()
            .await
            .map_err(|error| ProviderError::InvalidResponse(format!("failed to decode models: {error}")))?;
        Ok(models.data.into_iter().map(|entry| entry.id).collect())
    }
}

async fn sleep_backoff(attempt: usize) {
    tokio::time::sleep(backoff_delay(attempt)).await;
}

fn mentions_unsupported_schema(text: &str) -> bool {
    let lowered = text.to_lowercase();
    lowered.contains("response_format")
        || lowered.contains("json_schema")
        || lowered.contains("structured output")
}

/// Remove markdown code fences some servers wrap JSON in.
fn strip_code_fence(content: &str) -> String {
    let trimmed = content.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest.trim_start_matches("json").trim_start();
        return rest.trim_end_matches("```").trim().to_string();
    }
    trimmed.to_string()
}
