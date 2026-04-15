use std::future::Future;
use std::time::Duration;

use crate::decision::LlmRoutingResponse;
use crate::error::RouterError;

/// Default timeout for a single LLM HTTP request.
///
/// If the remote endpoint does not respond within this window the request is
/// cancelled and `RouterError::LlmRequest` is returned so the router loop can
/// continue with the next event.
pub const LLM_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Abstraction over the LLM HTTP call that produces a routing decision.
///
/// In production this is backed by an OpenAI-compatible HTTP endpoint
/// (`OpenAiCompatClient`). In tests the `MockLlmClient` returns canned
/// `LlmRoutingResponse` values without any network I/O.
pub trait LlmClient: Send + Sync + Clone + 'static {
    fn complete(
        &self,
        prompt: String,
    ) -> impl Future<Output = Result<LlmRoutingResponse, RouterError>> + Send;
}

// ── Configuration ─────────────────────────────────────────────────────────────

/// Connection parameters for an OpenAI-compatible chat-completion endpoint.
///
/// xAI, Anthropic (via the OpenAI-compatible shim), and OpenAI itself all
/// speak this protocol.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// Base URL of the API, e.g. `"https://api.x.ai/v1"`.
    pub api_url: String,
    /// Bearer token for the `Authorization` header.
    pub api_key: String,
    /// Model identifier, e.g. `"grok-3-mini"` or `"claude-3-5-haiku-20241022"`.
    pub model: String,
}

// ── Production implementation ─────────────────────────────────────────────────

/// Sends a single system prompt to an OpenAI-compatible `/chat/completions`
/// endpoint and returns the parsed [`LlmRoutingResponse`].
#[derive(Clone)]
pub struct OpenAiCompatClient {
    http: reqwest::Client,
    config: LlmConfig,
}

impl OpenAiCompatClient {
    pub fn new(http: reqwest::Client, config: LlmConfig) -> Self {
        Self { http, config }
    }
}

impl LlmClient for OpenAiCompatClient {
    async fn complete(&self, prompt: String) -> Result<LlmRoutingResponse, RouterError> {
        let url = format!("{}/chat/completions", self.config.api_url);

        let body = serde_json::json!({
            "model": self.config.model,
            "messages": [
                { "role": "system", "content": prompt }
            ],
            "response_format": { "type": "json_object" }
        });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.config.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| RouterError::LlmRequest(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(RouterError::LlmRequest(format!(
                "HTTP {status}: {text}"
            )));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| RouterError::LlmParse(e.to_string()))?;

        // Extract the first choice's message content.
        let content = json
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                RouterError::LlmParse(format!(
                    "missing choices[0].message.content in: {json}"
                ))
            })?;

        serde_json::from_str::<LlmRoutingResponse>(content)
            .map_err(|e| RouterError::LlmParse(format!("{e}: {content}")))
    }
}

// ── Unit tests for OpenAiCompatClient ────────────────────────────────────────

#[cfg(test)]
mod client_tests {
    use super::*;
    use axum::{Router, http::StatusCode, routing::post};
    use std::future::IntoFuture as _;
    use std::sync::Arc;

    /// Spawn an `axum` server that returns the given JSON body and HTTP status.
    /// Returns the bound socket address.
    async fn spawn_mock_llm(status: StatusCode, body: String) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock LLM");
        let addr = listener.local_addr().unwrap();
        let body = Arc::new(body);

        let app = Router::new().route(
            "/chat/completions",
            post(move || {
                let b = Arc::clone(&body);
                async move {
                    (status, [("content-type", "application/json")], (*b).clone())
                }
            }),
        );

        tokio::spawn(axum::serve(listener, app).into_future());
        addr
    }

    fn client(addr: std::net::SocketAddr) -> OpenAiCompatClient {
        OpenAiCompatClient::new(
            reqwest::Client::new(),
            LlmConfig {
                api_url: format!("http://{addr}"),
                api_key: "test-key".into(),
                model: "test-model".into(),
            },
        )
    }

    fn choices_body(content: &str) -> String {
        serde_json::json!({
            "choices": [{"message": {"content": content}}]
        })
        .to_string()
    }

    #[tokio::test]
    async fn complete_returns_routed_on_valid_response() {
        let content = r#"{"status":"routed","agent_type":"PrActor","entity_key":"owner/repo/1","reasoning":"test"}"#;
        let addr = spawn_mock_llm(StatusCode::OK, choices_body(content)).await;
        let result = client(addr).complete("test prompt".into()).await.unwrap();
        assert!(matches!(
            result,
            LlmRoutingResponse::Routed { agent_type, .. } if agent_type == "PrActor"
        ));
    }

    #[tokio::test]
    async fn complete_returns_unroutable_on_valid_response() {
        let content = r#"{"status":"unroutable","reasoning":"no match"}"#;
        let addr = spawn_mock_llm(StatusCode::OK, choices_body(content)).await;
        let result = client(addr).complete("test prompt".into()).await.unwrap();
        assert!(matches!(result, LlmRoutingResponse::Unroutable { .. }));
    }

    #[tokio::test]
    async fn complete_returns_llm_request_error_on_non_2xx() {
        let addr = spawn_mock_llm(
            StatusCode::INTERNAL_SERVER_ERROR,
            r#"{"error":"oops"}"#.to_string(),
        )
        .await;
        let err = client(addr).complete("test".into()).await.unwrap_err();
        assert!(matches!(err, RouterError::LlmRequest(_)));
    }

    #[tokio::test]
    async fn complete_returns_llm_parse_error_when_choices_missing() {
        let addr = spawn_mock_llm(
            StatusCode::OK,
            r#"{"id":"x","choices":[]}"#.to_string(),
        )
        .await;
        let err = client(addr).complete("test".into()).await.unwrap_err();
        assert!(matches!(err, RouterError::LlmParse(_)));
    }

    #[tokio::test]
    async fn complete_returns_llm_parse_error_when_content_not_json() {
        let body_start = r#"{"choices": [{"message": {"content": "this is plain text, not json"}}]}"#;
        let addr = spawn_mock_llm(StatusCode::OK, body_start.to_string()).await;
        let err = client(addr).complete("test".into()).await.unwrap_err();
        assert!(matches!(err, RouterError::LlmParse(_)));
    }

    /// `choices[0]` is present but missing the `"message"` key entirely.
    /// `pointer("/choices/0/message/content")` returns `None` → `LlmParse`.
    #[tokio::test]
    async fn complete_returns_llm_parse_error_when_message_key_missing() {
        let addr = spawn_mock_llm(
            StatusCode::OK,
            r#"{"choices": [{"index": 0, "finish_reason": "stop"}]}"#.to_string(),
        )
        .await;
        let err = client(addr).complete("test".into()).await.unwrap_err();
        assert!(matches!(err, RouterError::LlmParse(_)));
    }

    /// `choices[0].message` is present but missing the `"content"` key.
    /// `pointer("/choices/0/message/content")` returns `None` → `LlmParse`.
    #[tokio::test]
    async fn complete_returns_llm_parse_error_when_content_key_missing() {
        let addr = spawn_mock_llm(
            StatusCode::OK,
            r#"{"choices": [{"message": {"role": "assistant"}}]}"#.to_string(),
        )
        .await;
        let err = client(addr).complete("test".into()).await.unwrap_err();
        assert!(matches!(err, RouterError::LlmParse(_)));
    }
}

// ── Mock implementation ───────────────────────────────────────────────────────

#[cfg(any(test, feature = "test-support"))]
pub mod mock {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Canned LLM client for unit tests.
    ///
    /// Each call to [`complete`][MockLlmClient::complete] pops the next queued
    /// response. If the queue is exhausted the call panics, which makes test
    /// failures obvious.
    #[derive(Clone, Default)]
    pub struct MockLlmClient {
        responses: Arc<Mutex<Vec<LlmRoutingResponse>>>,
        /// Records every prompt received, in order.
        pub prompts: Arc<Mutex<Vec<String>>>,
    }

    impl MockLlmClient {
        pub fn new() -> Self {
            Self::default()
        }

        /// Enqueue one response that will be returned by the next `complete` call.
        pub fn push_response(&self, response: LlmRoutingResponse) {
            self.responses.lock().unwrap().push(response);
        }

        /// Drain and return all prompts received so far.
        pub fn take_prompts(&self) -> Vec<String> {
            self.prompts.lock().unwrap().drain(..).collect()
        }
    }

    impl LlmClient for MockLlmClient {
        async fn complete(&self, prompt: String) -> Result<LlmRoutingResponse, RouterError> {
            self.prompts.lock().unwrap().push(prompt);
            let response = self
                .responses
                .lock()
                .unwrap()
                .remove(0); // panics on underflow — intentional
            Ok(response)
        }
    }
}
