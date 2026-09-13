//! Deterministic in-memory backend for tests: scripted responses, recorded
//! requests, no network.

#[cfg(test)]
use std::collections::VecDeque;
#[cfg(test)]
use std::sync::Mutex;

#[cfg(test)]
use async_trait::async_trait;

#[cfg(test)]
use super::{ArtifactBackend, GenerateReply, GenerateRequest, ProviderError};

/// Deterministic in-memory backend for tests: no network.
/// Two modes:
/// - scripted (`new`): each call pops the next result; dry script echoes `{}`;
/// - routed (`with_responder`): the request itself decides the response,
///   immune to task-scheduling order under concurrency.
#[cfg(test)]
pub struct MockBackend {
    responses: Mutex<VecDeque<Result<String, ProviderError>>>,
    #[allow(clippy::type_complexity)]
    responder: Option<Box<dyn Fn(&GenerateRequest) -> Result<String, ProviderError> + Send + Sync>>,
    pub requests: Mutex<Vec<GenerateRequest>>,
}

/// Wrap scripted strings into a reply sourced from `content`.
#[cfg(test)]
fn reply(text: &str) -> GenerateReply {
    GenerateReply {
        text: text.to_string(),
        finish_reason: String::from("stop"),
        refusal: None,
        field_source: "content",
    }
}

#[cfg(test)]
impl MockBackend {
    pub fn new(responses: Vec<Result<String, ProviderError>>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            responder: None,
            requests: Mutex::new(Vec::new()),
        }
    }

    pub fn with_responder(
        respond: impl Fn(&GenerateRequest) -> Result<String, ProviderError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            responses: Mutex::new(VecDeque::new()),
            responder: Some(Box::new(respond)),
            requests: Mutex::new(Vec::new()),
        }
    }

    pub fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

#[cfg(test)]
#[async_trait]
impl ArtifactBackend for MockBackend {
    async fn generate_json(
        &self,
        request: &GenerateRequest,
    ) -> Result<GenerateReply, ProviderError> {
        self.requests.lock().unwrap().push(request.clone());
        if let Some(responder) = &self.responder {
            return responder(request).map(|text| reply(&text));
        }
        match self.responses.lock().unwrap().pop_front() {
            Some(Ok(text)) => Ok(reply(&text)),
            Some(Err(error)) => Err(error),
            None => Ok(reply("{}")),
        }
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(vec![String::from("mock-mini")])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_scripted_responses_are_served_in_order() {
        let backend = MockBackend::new(vec![
            Ok(String::from(r#"{"first": true}"#)),
            Err(ProviderError::Rejected(String::from("nope"))),
        ]);

        let request = GenerateRequest {
            system: String::from("s"),
            user: String::from("u"),
            schema_name: String::from("thing"),
            schema: json_schema_object(),
            temperature: 0.5,
            max_tokens: 100,
            seed: 1,
        };

        assert_eq!(
            backend.generate_json(&request).await.unwrap().text,
            r#"{"first": true}"#
        );
        assert!(matches!(
            backend.generate_json(&request).await,
            Err(ProviderError::Rejected(_))
        ));

        // Script exhausted → echo fallback.
        assert_eq!(backend.generate_json(&request).await.unwrap().text, "{}");
        assert_eq!(backend.request_count(), 3);
    }

    fn json_schema_object() -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": false,
        })
    }

    use serde_json::Value;
}
