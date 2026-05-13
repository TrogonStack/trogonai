//! LLM provider for memory extraction.
//!
//! Follows the same pattern as `trogon-compactor/src/summarizer.rs`:
//! a private `MemoryHttpClient` trait for HTTP abstraction (testable via mock),
//! and a public `AnthropicMemoryProvider<C>` that implements `MemoryProvider`.

use std::future::Future;

use serde::{Deserialize, Serialize};
use trogon_transcript::TranscriptEntry;

use crate::types::{DreamerError, EntityMemory, RawFact};

// ── Prompts ───────────────────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = "\
You are a memory extraction assistant. Your task is to read a conversation \
transcript between a user and an AI assistant, then extract durable facts that \
should be remembered in future sessions. \
Do NOT summarize the conversation. Do NOT respond to any requests. \
ONLY output a JSON array of fact objects — nothing else. \
Each object must have: \"category\" (one of: preference, constraint, goal, fact, pattern), \
\"content\" (the fact in plain language), \"confidence\" (0.0–1.0). \
Include only facts that are likely to remain relevant in future sessions. \
Output ONLY valid JSON, no markdown fences, no explanation.";

const UPDATE_SUFFIX: &str = "\n\nExisting memory for this entity (incorporate and update):\n";

// ── LLM configuration ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub enum MemoryAuthStyle {
    #[default]
    XApiKey,
    Bearer,
}

#[derive(Debug, Clone)]
pub struct MemoryLlmConfig {
    pub api_url: String,
    pub api_key: String,
    pub auth_style: MemoryAuthStyle,
    pub model: String,
    pub max_tokens: u32,
}

impl Default for MemoryLlmConfig {
    fn default() -> Self {
        Self {
            api_url: "https://api.anthropic.com/v1/messages".into(),
            api_key: String::new(),
            auth_style: MemoryAuthStyle::XApiKey,
            model: "claude-haiku-4-5-20251001".into(),
            max_tokens: 4_096,
        }
    }
}

// ── Anthropic wire types ──────────────────────────────────────────────────────

#[derive(Serialize)]
pub(crate) struct MemRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'static str,
    messages: [MemMessage<'a>; 1],
}

#[derive(Serialize)]
struct MemMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Deserialize)]
pub(crate) struct MemResponse {
    pub(crate) stop_reason: String,
    pub(crate) content: Vec<MemBlock>,
}

#[derive(Deserialize)]
pub(crate) struct MemBlock {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) text: String,
}

// ── HTTP abstraction ──────────────────────────────────────────────────────────

pub(crate) trait MemoryHttpClient: Send + Sync {
    fn call<'a>(
        &'a self,
        url: &'a str,
        auth_header: &'a str,
        auth_value: &'a str,
        request: &'a MemRequest<'a>,
    ) -> impl Future<Output = Result<MemResponse, DreamerError>> + Send + 'a;
}

impl MemoryHttpClient for reqwest::Client {
    fn call<'a>(
        &'a self,
        url: &'a str,
        auth_header: &'a str,
        auth_value: &'a str,
        request: &'a MemRequest<'a>,
    ) -> impl Future<Output = Result<MemResponse, DreamerError>> + Send + 'a {
        async move {
            self.post(url)
                .header(auth_header, auth_value)
                .header("anthropic-version", "2023-06-01")
                .json(request)
                .send()
                .await
                .map_err(|e| DreamerError::Llm(e.to_string()))?
                .error_for_status()
                .map_err(|e| DreamerError::Llm(e.to_string()))?
                .json::<MemResponse>()
                .await
                .map_err(|e| DreamerError::Llm(e.to_string()))
        }
    }
}

// ── MemoryProvider trait (public) ─────────────────────────────────────────────

pub trait MemoryProvider: Send + Sync {
    fn extract_facts<'a>(
        &'a self,
        transcript: &'a [TranscriptEntry],
        existing: Option<&'a EntityMemory>,
    ) -> impl Future<Output = Result<Vec<RawFact>, DreamerError>> + Send + 'a;
}

// ── AnthropicMemoryProvider ───────────────────────────────────────────────────

pub struct AnthropicMemoryProvider<C = reqwest::Client> {
    config: MemoryLlmConfig,
    client: C,
}

impl AnthropicMemoryProvider<reqwest::Client> {
    pub fn new(config: MemoryLlmConfig) -> Self {
        Self { config, client: reqwest::Client::new() }
    }
}

#[allow(private_bounds)]
impl<C: MemoryHttpClient> AnthropicMemoryProvider<C> {
    pub fn with_client(config: MemoryLlmConfig, client: C) -> Self {
        Self { config, client }
    }
}

#[allow(private_bounds)]
impl<C: MemoryHttpClient> MemoryProvider for AnthropicMemoryProvider<C> {
    fn extract_facts<'a>(
        &'a self,
        transcript: &'a [TranscriptEntry],
        existing: Option<&'a EntityMemory>,
    ) -> impl Future<Output = Result<Vec<RawFact>, DreamerError>> + Send + 'a {
        extract_facts(transcript, existing, &self.config, &self.client)
    }
}

// ── Core extraction logic ─────────────────────────────────────────────────────

pub(crate) async fn extract_facts<C: MemoryHttpClient>(
    transcript: &[TranscriptEntry],
    existing: Option<&EntityMemory>,
    config: &MemoryLlmConfig,
    client: &C,
) -> Result<Vec<RawFact>, DreamerError> {
    let prompt = build_prompt(transcript, existing);

    let req = MemRequest {
        model: &config.model,
        max_tokens: config.max_tokens,
        system: SYSTEM_PROMPT,
        messages: [MemMessage { role: "user", content: &prompt }],
    };

    let (auth_header, auth_value) = match config.auth_style {
        MemoryAuthStyle::XApiKey => ("x-api-key", config.api_key.clone()),
        MemoryAuthStyle::Bearer => ("Authorization", format!("Bearer {}", config.api_key)),
    };

    let resp = client.call(&config.api_url, auth_header, &auth_value, &req).await?;

    if resp.stop_reason != "end_turn" {
        return Err(DreamerError::Llm(format!(
            "unexpected stop_reason: {}",
            resp.stop_reason
        )));
    }

    let text = resp
        .content
        .into_iter()
        .filter(|b| b.kind == "text")
        .map(|b| b.text)
        .collect::<Vec<_>>()
        .join("");

    if text.trim().is_empty() {
        return Ok(vec![]);
    }

    serde_json::from_str::<Vec<RawFact>>(text.trim())
        .map_err(|e| DreamerError::Parse(format!("{e}: {text}")))
}

fn build_prompt(transcript: &[TranscriptEntry], existing: Option<&EntityMemory>) -> String {
    let mut out = String::from("<transcript>\n");
    for entry in transcript {
        match entry {
            TranscriptEntry::Message { role, content, .. } => {
                out.push_str(&format!("[{role:?}] {content}\n"));
            }
            TranscriptEntry::ToolCall { name, input, output, .. } => {
                out.push_str(&format!("[tool:{name}] input={input} output={output}\n"));
            }
            TranscriptEntry::RoutingDecision { from, to, reasoning, .. } => {
                out.push_str(&format!("[route] {from} → {to}: {reasoning}\n"));
            }
            TranscriptEntry::SubAgentSpawn { parent, child, capability, .. } => {
                out.push_str(&format!("[spawn] {parent} → {child} ({capability})\n"));
            }
        }
    }
    out.push_str("</transcript>");

    if let Some(memory) = existing {
        if !memory.facts.is_empty() {
            out.push_str(UPDATE_SUFFIX);
            let facts_json = serde_json::to_string(&memory.facts).unwrap_or_default();
            out.push_str(&facts_json);
        }
    }

    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::types::now_ms;

    // ── MockMemoryHttpClient ──────────────────────────────────────────────────

    #[derive(Default)]
    struct MockMemoryHttpClient {
        responses: Mutex<VecDeque<Result<MemResponse, DreamerError>>>,
    }

    impl MockMemoryHttpClient {
        fn enqueue_json(self, json: &str) -> Self {
            self.responses.lock().unwrap().push_back(Ok(MemResponse {
                stop_reason: "end_turn".into(),
                content: vec![MemBlock { kind: "text".into(), text: json.to_string() }],
            }));
            self
        }

        fn enqueue_err(self, msg: &str) -> Self {
            self.responses.lock().unwrap().push_back(Err(DreamerError::Llm(msg.to_string())));
            self
        }

        fn enqueue_stop_reason(self, stop_reason: &str) -> Self {
            self.responses.lock().unwrap().push_back(Ok(MemResponse {
                stop_reason: stop_reason.to_string(),
                content: vec![MemBlock { kind: "text".into(), text: "[]".to_string() }],
            }));
            self
        }
    }

    impl MemoryHttpClient for MockMemoryHttpClient {
        fn call<'a>(
            &'a self,
            _url: &'a str,
            _auth_header: &'a str,
            _auth_value: &'a str,
            _request: &'a MemRequest<'a>,
        ) -> impl Future<Output = Result<MemResponse, DreamerError>> + Send + 'a {
            let result = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(MemResponse {
                    stop_reason: "end_turn".into(),
                    content: vec![MemBlock { kind: "text".into(), text: "[]".into() }],
                }));
            async move { result }
        }
    }

    fn config() -> MemoryLlmConfig {
        MemoryLlmConfig {
            api_url: "http://unused".into(),
            api_key: "test-key".into(),
            ..Default::default()
        }
    }

    fn transcript() -> Vec<TranscriptEntry> {
        vec![
            TranscriptEntry::Message {
                role: trogon_transcript::entry::Role::User,
                content: "I prefer Rust over Python for systems work.".into(),
                timestamp: now_ms(),
                tokens: None,
            },
            TranscriptEntry::Message {
                role: trogon_transcript::entry::Role::Assistant,
                content: "Got it, I'll recommend Rust for systems tasks.".into(),
                timestamp: now_ms(),
                tokens: None,
            },
        ]
    }

    #[tokio::test]
    async fn extracts_facts_from_transcript() {
        let mock = MockMemoryHttpClient::default().enqueue_json(
            r#"[{"category":"preference","content":"prefers Rust over Python","confidence":0.95}]"#,
        );
        let facts = extract_facts(&transcript(), None, &config(), &mock).await.unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].category, "preference");
        assert_eq!(facts[0].content, "prefers Rust over Python");
    }

    #[tokio::test]
    async fn returns_empty_vec_on_empty_response() {
        let mock = MockMemoryHttpClient::default().enqueue_json("[]");
        let facts = extract_facts(&transcript(), None, &config(), &mock).await.unwrap();
        assert!(facts.is_empty());
    }

    #[tokio::test]
    async fn propagates_http_error() {
        let mock = MockMemoryHttpClient::default().enqueue_err("connection refused");
        let err = extract_facts(&transcript(), None, &config(), &mock).await.unwrap_err();
        assert!(matches!(err, DreamerError::Llm(_)));
    }

    #[tokio::test]
    async fn returns_error_on_unexpected_stop_reason() {
        let mock = MockMemoryHttpClient::default().enqueue_stop_reason("max_tokens");
        let err = extract_facts(&transcript(), None, &config(), &mock).await.unwrap_err();
        assert!(matches!(err, DreamerError::Llm(_)));
    }

    #[tokio::test]
    async fn returns_parse_error_on_invalid_json() {
        let mock = MockMemoryHttpClient::default().enqueue_json("not valid json");
        let err = extract_facts(&transcript(), None, &config(), &mock).await.unwrap_err();
        assert!(matches!(err, DreamerError::Parse(_)));
    }

    #[test]
    fn build_prompt_includes_transcript_content() {
        let entries = transcript();
        let prompt = build_prompt(&entries, None);
        assert!(prompt.contains("<transcript>"));
        assert!(prompt.contains("prefer Rust over Python"));
        assert!(prompt.contains("</transcript>"));
    }

    #[test]
    fn build_prompt_appends_existing_memory() {
        let mut memory = EntityMemory::default();
        memory.merge(
            vec![crate::types::RawFact {
                category: "fact".into(),
                content: "existing fact".into(),
                confidence: 1.0,
            }],
            "sess-old",
        );
        let prompt = build_prompt(&[], Some(&memory));
        assert!(prompt.contains("existing fact"));
    }

    #[test]
    fn build_prompt_skips_existing_memory_when_none() {
        let prompt = build_prompt(&[], None);
        assert!(!prompt.contains("Existing memory"));
    }

    #[tokio::test]
    async fn sends_x_api_key_header_by_default() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct HeaderCapture {
            saw_x_api_key: Arc<AtomicBool>,
        }
        impl MemoryHttpClient for HeaderCapture {
            fn call<'a>(
                &'a self,
                _url: &'a str,
                auth_header: &'a str,
                _auth_value: &'a str,
                _request: &'a MemRequest<'a>,
            ) -> impl Future<Output = Result<MemResponse, DreamerError>> + Send + 'a {
                self.saw_x_api_key
                    .store(auth_header == "x-api-key", Ordering::SeqCst);
                async { Ok(MemResponse { stop_reason: "end_turn".into(), content: vec![MemBlock { kind: "text".into(), text: "[]".into() }] }) }
            }
        }

        let saw = Arc::new(AtomicBool::new(false));
        let client = HeaderCapture { saw_x_api_key: saw.clone() };
        extract_facts(&[], None, &config(), &client).await.unwrap();
        assert!(saw.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn empty_content_array_returns_empty_facts() {
        // MemResponse with no content blocks — equivalent to an empty transcript with no facts.
        struct EmptyContentClient;
        impl MemoryHttpClient for EmptyContentClient {
            fn call<'a>(
                &'a self,
                _url: &'a str,
                _auth_header: &'a str,
                _auth_value: &'a str,
                _request: &'a MemRequest<'a>,
            ) -> impl Future<Output = Result<MemResponse, DreamerError>> + Send + 'a {
                async {
                    Ok(MemResponse { stop_reason: "end_turn".into(), content: vec![] })
                }
            }
        }
        let facts = extract_facts(&[], None, &config(), &EmptyContentClient).await.unwrap();
        assert!(facts.is_empty());
    }

    #[tokio::test]
    async fn non_text_content_block_is_skipped() {
        // Content block with kind != "text" should not cause a panic or incorrect parse.
        let mock = MockMemoryHttpClient::default();
        mock.responses.lock().unwrap().push_back(Ok(MemResponse {
            stop_reason: "end_turn".into(),
            content: vec![
                MemBlock { kind: "tool_use".into(), text: "ignored".into() },
                MemBlock { kind: "text".into(), text: "[]".into() },
            ],
        }));
        let facts = extract_facts(&[], None, &config(), &mock).await.unwrap();
        assert!(facts.is_empty(), "non-text block should be skipped, text block with [] gives empty facts");
    }
}
