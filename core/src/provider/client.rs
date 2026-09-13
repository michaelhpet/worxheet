//! OpenAI-compatible chat-completions client with retry/backoff.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use super::{ArtifactBackend, GenerateReply, GenerateRequest, ProviderError};

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);
const MAX_ATTEMPTS: usize = 4;

fn backoff_delay(attempt: usize) -> std::time::Duration {
    std::time::Duration::from_millis(700 * (1 << attempt.min(3)) / 2 + 200)
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: Option<String>,
    /// Alternate text location on thinking-model deployments.
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    refusal: Option<String>,
}

impl ChatMessage {
    /// Ordinary content first, then reasoning fallbacks.
    fn effective_text(&self) -> (&str, &'static str) {
        for (field, name) in [
            (self.content.as_deref(), "content"),
            (self.reasoning.as_deref(), "reasoning"),
            (self.reasoning_content.as_deref(), "reasoning_content"),
        ] {
            if let Some(text) = field {
                if !text.trim().is_empty() {
                    return (text, name);
                }
            }
        }
        ("", "content")
    }
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

pub struct OpenAiClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    model: String,
    reasoning_effort: Option<String>,
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
            reasoning_effort: None,
        }
    }

    pub fn with_reasoning_effort(mut self, reasoning_effort: Option<String>) -> Self {
        self.reasoning_effort = reasoning_effort;
        self
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
    async fn generate_json(
        &self,
        request: &GenerateRequest,
    ) -> Result<GenerateReply, ProviderError> {
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
        if let Some(reasoning_effort) = &self.reasoning_effort {
            body["reasoning_effort"] = json!(reasoning_effort);
        }
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
                stripped
                    .as_object_mut()
                    .map(|map| map.remove("response_format"));
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
                last_error = ProviderError::RateLimited {
                    retry_after,
                    message,
                };
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
                // Fall back to prompt-only JSON when structured outputs are rejected.
                if structured && mentions_unsupported_schema(&text) {
                    structured = false;
                    continue;
                }
                return Err(ProviderError::Rejected(format!("{status}: {text}")));
            }

            let completion: ChatCompletionResponse = response.json().await.map_err(|error| {
                ProviderError::InvalidResponse(format!("failed to decode completion: {error}"))
            })?;

            let choice = completion.choices.first().ok_or_else(|| {
                ProviderError::InvalidResponse(String::from("completion had no choices"))
            })?;
            let message = &choice.message;
            let (text, field_source) = message.effective_text();

            return Ok(GenerateReply {
                text: strip_code_fence(text),
                finish_reason: choice.finish_reason.clone().unwrap_or_default(),
                refusal: message.refusal.clone(),
                field_source,
            });
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
            return Err(ProviderError::Rejected(format!(
                "model listing failed ({status})"
            )));
        }
        if status.is_server_error() {
            return Err(ProviderError::Server(format!(
                "model listing failed ({status})"
            )));
        }

        let models: ModelsResponse = response.json().await.map_err(|error| {
            ProviderError::InvalidResponse(format!("failed to decode models: {error}"))
        })?;
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

fn strip_code_fence(content: &str) -> String {
    let trimmed = content.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest.trim_start_matches("json").trim_start();
        return rest.trim_end_matches("```").trim().to_string();
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(
        content: Option<&str>,
        reasoning: Option<&str>,
        reasoning_content: Option<&str>,
    ) -> ChatMessage {
        ChatMessage {
            content: content.map(String::from),
            reasoning: reasoning.map(String::from),
            reasoning_content: reasoning_content.map(String::from),
            refusal: None,
        }
    }

    #[test]
    fn test_effective_text_prefers_content() {
        let msg = message(Some(r#"{"ok":true}"#), None, None);
        assert_eq!(msg.effective_text(), (r#"{"ok":true}"#, "content"));
    }

    #[test]
    fn test_effective_text_falls_back_to_reasoning_when_content_empty() {
        let msg = message(None, Some(r#"[answer]"#), None);
        assert_eq!(msg.effective_text(), ("[answer]", "reasoning"));
        let msg = message(Some(""), Some("[answer]"), Some("[also]"));
        assert_eq!(msg.effective_text(), ("[answer]", "reasoning"));
    }

    #[test]
    fn test_effective_text_falls_back_to_reasoning_content() {
        let msg = message(None, None, Some(r#"[answer]"#));
        assert_eq!(msg.effective_text(), ("[answer]", "reasoning_content"));
    }

    #[test]
    fn test_effective_text_empty_when_nothing_present() {
        let msg = message(None, None, None);
        assert_eq!(msg.effective_text(), ("", "content"));
    }

    #[test]
    fn test_strip_code_fence() {
        assert_eq!(strip_code_fence("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_code_fence("plain"), "plain");
    }
}
