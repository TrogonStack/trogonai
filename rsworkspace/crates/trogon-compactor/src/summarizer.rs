//! Calls the Anthropic API to produce a structured conversation summary.
//!
//! Uses a single non-streaming request with a dedicated system prompt that
//! prevents the model from continuing the conversation.  If a previous summary
//! exists, an update prompt is used to merge new information incrementally.

use std::future::Future;

use serde::{Deserialize, Serialize};

use crate::error::CompactorError;
use crate::serializer::serialize_for_prompt;
use crate::types::Message;

// ── Prompts ───────────────────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = "\
You are a context summarization assistant. Your task is to read a \
conversation between a user and an AI coding assistant, then produce \
a structured summary. \
Do NOT continue the conversation. Do NOT respond to any questions. \
ONLY output the structured summary — nothing else.";

const INITIAL_PROMPT: &str = "\
Create a structured context checkpoint summary using EXACTLY this format:

## Goal
[What is the user trying to accomplish?]

## Constraints & Preferences
- [Any constraints or preferences mentioned]

## Progress
### Done
- [x] [Completed tasks]
### In Progress
- [ ] [Current work]
### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what comes next]

## Critical Context
- [File paths, function names, error messages, or data needed to continue]";

const UPDATE_PROMPT: &str = "\
Update the existing summary below with new information from the conversation.

RULES:
- PRESERVE all existing information unless it has been superseded.
- ADD new progress and decisions from the new messages.
- UPDATE the Progress section: move items from In Progress → Done when completed.
- PRESERVE exact file paths, function names, and error messages.
- Output ONLY the updated structured summary — nothing else.";

// ── LLM configuration ─────────────────────────────────────────────────────────

/// Authentication style for the LLM endpoint.
#[derive(Debug, Clone, Default)]
pub enum AuthStyle {
    /// Direct Anthropic API — sends `x-api-key: {key}`.
    #[default]
    XApiKey,
    /// Via `trogon-secret-proxy` — sends `Authorization: Bearer {token}`.
    /// This is the standard auth style for all trogon services.
    Bearer,
}

/// Connection details for the summarization LLM.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// Full messages endpoint URL.
    ///
    /// - Direct Anthropic: `https://api.anthropic.com/v1/messages`
    /// - Via trogon-secret-proxy: `{PROXY_URL}/anthropic/v1/messages`
    pub api_url: String,
    /// API key or proxy bearer token.
    pub api_key: String,
    /// How the key is sent. Use [`AuthStyle::Bearer`] when routing through
    /// `trogon-secret-proxy` (the standard in production).
    pub auth_style: AuthStyle,
    /// Model to use. Prefer a fast, cost-efficient model (e.g. Haiku).
    pub model: String,
    /// Maximum tokens for the summary output.
    pub max_summary_tokens: u32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            api_url: "https://api.anthropic.com/v1/messages".into(),
            api_key: String::new(),
            auth_style: AuthStyle::XApiKey,
            model: "claude-haiku-4-5-20251001".into(),
            max_summary_tokens: 8_192,
        }
    }
}

// ── Anthropic wire types ──────────────────────────────────────────────────────

#[derive(Serialize)]
pub(crate) struct SumRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'static str,
    messages: [SumMessage<'a>; 1],
}

#[derive(Serialize)]
struct SumMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Deserialize)]
pub(crate) struct SumResponse {
    pub(crate) stop_reason: String,
    pub(crate) content: Vec<SumBlock>,
}

#[derive(Deserialize)]
pub(crate) struct SumBlock {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) text: String,
}

// ── HTTP abstraction ──────────────────────────────────────────────────────────

/// Sends a single POST request to the Anthropic messages endpoint.
///
/// Concrete implementations use `reqwest`; unit tests use [`MockSumClient`].
pub(crate) trait SummarizationClient: Send + Sync {
    fn call<'a>(
        &'a self,
        url: &'a str,
        auth_header: &'a str,
        auth_value: &'a str,
        request: &'a SumRequest<'a>,
    ) -> impl Future<Output = Result<SumResponse, CompactorError>> + Send + 'a;
}

impl SummarizationClient for reqwest::Client {
    fn call<'a>(
        &'a self,
        url: &'a str,
        auth_header: &'a str,
        auth_value: &'a str,
        request: &'a SumRequest<'a>,
    ) -> impl Future<Output = Result<SumResponse, CompactorError>> + Send + 'a {
        async move {
            Ok(self
                .post(url)
                .header(auth_header, auth_value)
                .header("anthropic-version", "2023-06-01")
                .json(request)
                .send()
                .await?
                .error_for_status()?
                .json::<SumResponse>()
                .await?)
        }
    }
}

// ── AnthropicLlmProvider ──────────────────────────────────────────────────────

/// Real [`LlmProvider`] that calls the Anthropic-compatible API.
///
/// Generic over `C: SummarizationClient` so unit tests can inject a mock;
/// defaults to `reqwest::Client` for production use.
pub struct AnthropicLlmProvider<C = reqwest::Client> {
    config: LlmConfig,
    client: C,
}

impl AnthropicLlmProvider<reqwest::Client> {
    pub fn new(config: LlmConfig) -> Self {
        Self { config, client: reqwest::Client::new() }
    }
}

#[allow(private_bounds)]
impl<C: SummarizationClient> AnthropicLlmProvider<C> {
    pub fn with_client(config: LlmConfig, client: C) -> Self {
        Self { config, client }
    }
}

#[allow(private_bounds)]
impl<C: SummarizationClient> crate::traits::LlmProvider for AnthropicLlmProvider<C> {
    fn generate_summary<'a>(
        &'a self,
        messages: &'a [crate::types::Message],
        previous_summary: Option<&'a str>,
    ) -> impl std::future::Future<Output = Result<String, crate::error::CompactorError>> + Send + 'a
    {
        generate_summary(messages, previous_summary, &self.config, &self.client)
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Calls the LLM to summarize `messages_to_summarize`.
///
/// If `previous_summary` is `Some`, an incremental update prompt is used so
/// the model merges new information rather than regenerating from scratch.
pub(crate) async fn generate_summary<C: SummarizationClient>(
    messages_to_summarize: &[Message],
    previous_summary: Option<&str>,
    config: &LlmConfig,
    client: &C,
) -> Result<String, CompactorError> {
    let conversation = serialize_for_prompt(messages_to_summarize);
    let prompt = build_prompt(&conversation, previous_summary);

    let req = SumRequest {
        model: &config.model,
        max_tokens: config.max_summary_tokens,
        system: SYSTEM_PROMPT,
        messages: [SumMessage {
            role: "user",
            content: &prompt,
        }],
    };

    let (auth_header, auth_value) = match config.auth_style {
        AuthStyle::XApiKey => ("x-api-key", config.api_key.clone()),
        AuthStyle::Bearer => ("Authorization", format!("Bearer {}", config.api_key)),
    };

    let resp = client.call(&config.api_url, auth_header, &auth_value, &req).await?;

    if resp.stop_reason != "end_turn" {
        return Err(CompactorError::UnexpectedStopReason(resp.stop_reason));
    }

    let text: String = resp
        .content
        .into_iter()
        .filter(|b| b.kind == "text")
        .map(|b| b.text)
        .collect::<Vec<_>>()
        .join("\n");

    if text.is_empty() {
        return Err(CompactorError::EmptyResponse);
    }

    Ok(text)
}

fn build_prompt(conversation: &str, previous_summary: Option<&str>) -> String {
    let mut out = format!("<conversation>\n{conversation}\n</conversation>\n\n");

    if let Some(prev) = previous_summary {
        out.push_str(&format!(
            "<previous-summary>\n{prev}\n</previous-summary>\n\n{UPDATE_PROMPT}"
        ));
    } else {
        out.push_str(INITIAL_PROMPT);
    }

    out
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::types::Message;

    // ── MockSumClient ─────────────────────────────────────────────────────────

    #[derive(Default)]
    struct MockSumClient {
        responses: Mutex<VecDeque<Result<SumResponse, CompactorError>>>,
        last_auth_header: Mutex<Option<String>>,
        last_auth_value: Mutex<Option<String>>,
    }

    impl MockSumClient {
        fn enqueue_ok(self, stop_reason: &str, blocks: Vec<SumBlock>) -> Self {
            self.responses.lock().unwrap().push_back(Ok(SumResponse {
                stop_reason: stop_reason.to_string(),
                content: blocks,
            }));
            self
        }

        fn enqueue_err(self, msg: impl Into<String>) -> Self {
            self.responses.lock().unwrap().push_back(Err(CompactorError::Http(msg.into())));
            self
        }

        fn last_auth_header(&self) -> Option<String> {
            self.last_auth_header.lock().unwrap().clone()
        }

        fn last_auth_value(&self) -> Option<String> {
            self.last_auth_value.lock().unwrap().clone()
        }

        fn text_block(text: &str) -> SumBlock {
            SumBlock { kind: "text".into(), text: text.into() }
        }

        fn tool_block() -> SumBlock {
            SumBlock { kind: "tool_use".into(), text: String::new() }
        }
    }

    impl SummarizationClient for MockSumClient {
        fn call<'a>(
            &'a self,
            _url: &'a str,
            auth_header: &'a str,
            auth_value: &'a str,
            _request: &'a SumRequest<'a>,
        ) -> impl Future<Output = Result<SumResponse, CompactorError>> + Send + 'a {
            *self.last_auth_header.lock().unwrap() = Some(auth_header.to_string());
            *self.last_auth_value.lock().unwrap() = Some(auth_value.to_string());
            let result = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(SumResponse { stop_reason: "end_turn".into(), content: vec![] }));
            async move { result }
        }
    }

    fn one_msg() -> Vec<Message> {
        vec![Message::user("hello")]
    }

    fn config() -> LlmConfig {
        LlmConfig {
            api_url: "http://unused".to_string(),
            api_key: "test-key".into(),
            auth_style: AuthStyle::XApiKey,
            model: "claude-test".into(),
            max_summary_tokens: 1_000,
        }
    }

    // ── build_prompt (pure) ───────────────────────────────────────────────────

    #[test]
    fn build_prompt_initial_contains_conversation() {
        let prompt = build_prompt("some conversation", None);
        assert!(prompt.contains("<conversation>"));
        assert!(prompt.contains("some conversation"));
        assert!(prompt.contains("## Goal"));
    }

    #[test]
    fn build_prompt_update_contains_previous_summary() {
        let prompt = build_prompt("new messages", Some("old summary"));
        assert!(prompt.contains("<previous-summary>"));
        assert!(prompt.contains("old summary"));
        assert!(prompt.contains("Update the existing summary"));
    }

    #[test]
    fn default_api_url_is_full_anthropic_messages_endpoint() {
        let config = LlmConfig::default();
        assert_eq!(config.api_url, "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn build_prompt_initial_and_update_use_different_suffixes() {
        let init   = build_prompt("conv", None);
        let update = build_prompt("conv", Some("prev"));
        assert_ne!(init, update, "initial and update prompts must differ");
        assert!(init.contains(INITIAL_PROMPT));
        assert!(update.contains(UPDATE_PROMPT));
    }

    #[test]
    fn build_prompt_wraps_conversation_in_tags() {
        let prompt = build_prompt("hello world", None);
        let start = prompt.find("<conversation>").unwrap();
        let end   = prompt.find("</conversation>").unwrap();
        let inner = &prompt[start..=end + "</conversation>".len() - 1];
        assert!(inner.contains("hello world"));
    }

    // ── generate_summary via MockSumClient ────────────────────────────────────

    #[tokio::test]
    async fn generate_summary_returns_text_on_success() {
        let mock = MockSumClient::default()
            .enqueue_ok("end_turn", vec![MockSumClient::text_block("## Summary\nDone.")]);
        let result = generate_summary(&one_msg(), None, &config(), &mock).await.unwrap();
        assert!(result.contains("## Summary"));
    }

    #[tokio::test]
    async fn generate_summary_returns_http_error_on_5xx() {
        let mock = MockSumClient::default().enqueue_err("500 Internal Server Error");
        let err = generate_summary(&one_msg(), None, &config(), &mock).await.unwrap_err();
        assert!(matches!(err, CompactorError::Http(_)));
    }

    #[tokio::test]
    async fn generate_summary_returns_unexpected_stop_reason() {
        let mock = MockSumClient::default()
            .enqueue_ok("max_tokens", vec![MockSumClient::text_block("partial")]);
        let err = generate_summary(&one_msg(), None, &config(), &mock).await.unwrap_err();
        assert!(matches!(err, CompactorError::UnexpectedStopReason(r) if r == "max_tokens"));
    }

    #[tokio::test]
    async fn generate_summary_returns_empty_response_when_no_text_blocks() {
        let mock = MockSumClient::default().enqueue_ok("end_turn", vec![]);
        let err = generate_summary(&one_msg(), None, &config(), &mock).await.unwrap_err();
        assert!(matches!(err, CompactorError::EmptyResponse));
    }

    #[tokio::test]
    async fn generate_summary_returns_http_error_on_malformed_json() {
        let mock = MockSumClient::default().enqueue_err("response body deserialization failed");
        let err = generate_summary(&one_msg(), None, &config(), &mock).await.unwrap_err();
        assert!(matches!(err, CompactorError::Http(_)));
    }

    #[tokio::test]
    async fn generate_summary_filters_non_text_content_blocks() {
        let mock = MockSumClient::default().enqueue_ok(
            "end_turn",
            vec![
                MockSumClient::tool_block(),
                MockSumClient::text_block("the summary"),
                MockSumClient::tool_block(),
            ],
        );
        let result = generate_summary(&one_msg(), None, &config(), &mock).await.unwrap();
        assert_eq!(result.trim(), "the summary");
    }

    #[tokio::test]
    async fn generate_summary_sends_x_api_key_header_for_anthropic_auth() {
        let mock = MockSumClient::default()
            .enqueue_ok("end_turn", vec![MockSumClient::text_block("summary text")]);
        let cfg = LlmConfig { api_key: "my-key".into(), ..config() };
        generate_summary(&one_msg(), None, &cfg, &mock).await.unwrap();
        assert_eq!(mock.last_auth_header().as_deref(), Some("x-api-key"));
        assert_eq!(mock.last_auth_value().as_deref(), Some("my-key"));
    }

    #[tokio::test]
    async fn generate_summary_sends_authorization_header_for_bearer_auth() {
        let mock = MockSumClient::default()
            .enqueue_ok("end_turn", vec![MockSumClient::text_block("summary text")]);
        let cfg = LlmConfig {
            api_key: "my-token".into(),
            auth_style: AuthStyle::Bearer,
            ..config()
        };
        generate_summary(&one_msg(), None, &cfg, &mock).await.unwrap();
        assert_eq!(mock.last_auth_header().as_deref(), Some("Authorization"));
        assert_eq!(mock.last_auth_value().as_deref(), Some("Bearer my-token"));
    }
}
