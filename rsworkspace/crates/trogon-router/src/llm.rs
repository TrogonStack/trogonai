use std::future::Future;

use crate::decision::LlmRoutingResponse;
use crate::error::RouterError;

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

// ── Mock implementation ───────────────────────────────────────────────────────

#[cfg(any(test, feature = "test-helpers"))]
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
