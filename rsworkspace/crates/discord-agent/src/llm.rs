//! LLM integration module
//!
//! Handles communication with Claude API for generating responses.

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Claude API configuration
#[derive(Debug, Clone)]
pub struct ClaudeConfig {
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f32,
    /// Base URL for the Claude API. Defaults to `https://api.anthropic.com`.
    /// Override in tests to point to a local mock server.
    pub base_url: String,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 1024,
            temperature: 1.0,
            base_url: "https://api.anthropic.com".to_string(),
        }
    }
}

/// Claude API client
#[derive(Clone)]
pub struct ClaudeClient {
    client: Client,
    config: ClaudeConfig,
}

impl ClaudeClient {
    /// Create a new Claude client
    pub fn new(config: ClaudeConfig) -> Self {
        Self {
            client: Client::new(),
            config,
        }
    }

    /// Generate a response from Claude with streaming.
    ///
    /// Sends each text delta through `chunk_tx` as it arrives and returns the
    /// full accumulated response when the stream is complete.
    pub async fn generate_response_streaming(
        &self,
        system_prompt: &str,
        user_message: &str,
        conversation_history: &[Message],
        chunk_tx: tokio::sync::mpsc::Sender<String>,
    ) -> Result<String> {
        // Convert to API messages, stripping internal fields like message_id
        let mut messages: Vec<ApiMessage> = conversation_history
            .iter()
            .map(|m| ApiMessage {
                role: m.role.clone(),
                content: m.content.clone(),
            })
            .collect();
        messages.push(ApiMessage {
            role: "user".to_string(),
            content: user_message.to_string(),
        });

        let request = ClaudeRequest {
            model: self.config.model.clone(),
            max_tokens: self.config.max_tokens,
            temperature: self.config.temperature,
            system: system_prompt.to_string(),
            messages,
            stream: true,
        };

        debug!("Sending streaming request to Claude API");

        let response = self
            .client
            .post(format!("{}/v1/messages", self.config.base_url))
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send streaming request to Claude API")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Claude API streaming error: {} - {}", status, error_text);
        }

        let mut accumulated = String::new();
        let mut line_buf = String::new();
        let mut response = response;

        while let Some(chunk) = response
            .chunk()
            .await
            .context("Failed to read streaming chunk")?
        {
            line_buf.push_str(&String::from_utf8_lossy(&chunk));

            // Process all complete lines in the buffer
            while let Some(pos) = line_buf.find('\n') {
                let line = line_buf[..pos].trim().to_string();
                line_buf = line_buf[pos + 1..].to_string();

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        break;
                    }
                    if let Ok(event) = serde_json::from_str::<SseEvent>(data) {
                        if event.event_type == "content_block_delta" {
                            if let Some(delta) = event.delta {
                                if delta.delta_type == "text_delta" {
                                    if let Some(text) = delta.text {
                                        accumulated.push_str(&text);
                                        // Best-effort send; ignore if receiver is gone
                                        let _ = chunk_tx.try_send(text);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        debug!(
            "Streaming complete: {} chars accumulated",
            accumulated.len()
        );
        Ok(accumulated)
    }
}

/// Message in conversation history (stored in KV and in-memory).
///
/// `message_id` holds the Discord message ID for user turns so that
/// edit and delete events can be matched precisely instead of by position.
/// It is omitted from serialization when absent to keep storage compact
/// and to stay backward-compatible with existing history without IDs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    /// Discord message ID — only present for messages sent via the gateway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<u64>,
}

/// Minimal message shape sent to the Claude API.
///
/// The Claude API is strict about unknown fields, so we convert from
/// `Message` to `ApiMessage` before building the HTTP request.
#[derive(Debug, Serialize)]
struct ApiMessage {
    role: String,
    content: String,
}

/// Claude API request
#[derive(Debug, Serialize)]
struct ClaudeRequest {
    model: String,
    max_tokens: u32,
    temperature: f32,
    system: String,
    messages: Vec<ApiMessage>,
    /// Always true: we only support streaming mode
    stream: bool,
}

// ── SSE streaming types ────────────────────────────────────────────────────

/// Top-level SSE event from the Anthropic streaming API
#[derive(Debug, Deserialize)]
struct SseEvent {
    #[serde(rename = "type")]
    event_type: String,
    delta: Option<SseDelta>,
}

/// Delta payload inside a `content_block_delta` event
#[derive(Debug, Deserialize)]
struct SseDelta {
    #[serde(rename = "type")]
    delta_type: String,
    text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    // ── SSE type deserialization ─────────────────────────────────────────────

    #[test]
    fn test_sse_text_delta_parsed_correctly() {
        let json = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let event: SseEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_type, "content_block_delta");
        let delta = event.delta.unwrap();
        assert_eq!(delta.delta_type, "text_delta");
        assert_eq!(delta.text.as_deref(), Some("Hello"));
    }

    #[test]
    fn test_sse_non_content_event_has_no_delta() {
        // message_start and similar lifecycle events carry no text delta
        let json = r#"{"type":"message_start","message":{"id":"msg_123"}}"#;
        let event: SseEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_type, "message_start");
        assert!(event.delta.is_none());
    }

    #[test]
    fn test_sse_input_json_delta_has_no_text() {
        // Tool-use deltas have type=input_json_delta and no text field
        let json = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#;
        let event: SseEvent = serde_json::from_str(json).unwrap();
        let delta = event.delta.unwrap();
        assert_eq!(delta.delta_type, "input_json_delta");
        assert!(delta.text.is_none());
    }

    #[test]
    fn test_sse_unknown_top_level_fields_do_not_panic() {
        // Claude API may add new top-level fields; we must not fail on them
        let json =
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#;
        let event: SseEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_type, "content_block_start");
        assert!(event.delta.is_none());
    }

    // ── Message serde ────────────────────────────────────────────────────────

    #[test]
    fn test_message_id_omitted_from_json_when_none() {
        let msg = Message {
            role: "user".to_string(),
            content: "hello".to_string(),
            message_id: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            !json.contains("message_id"),
            "message_id should be absent when None, got: {}",
            json
        );
    }

    #[test]
    fn test_message_id_roundtrips_when_some() {
        let msg = Message {
            role: "assistant".to_string(),
            content: "hi there".to_string(),
            message_id: Some(99999),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let restored: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.role, "assistant");
        assert_eq!(restored.message_id, Some(99999));
    }

    // ── Full streaming integration with mock HTTP server ─────────────────────

    #[tokio::test]
    async fn test_generate_response_streaming_accumulates_text_deltas() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        // Minimal SSE response: two text deltas then [DONE]
        let sse_body = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"}}\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n",
            "data: {\"type\":\"message_stop\"}\n",
            "data: [DONE]\n",
        );

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "test-key"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse_body),
            )
            .expect(1)
            .mount(&server)
            .await;

        let config = ClaudeConfig {
            api_key: "test-key".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 100,
            temperature: 1.0,
            base_url: server.uri(),
        };
        let client = ClaudeClient::new(config);

        let (tx, mut rx) = mpsc::channel::<String>(16);
        let result = client
            .generate_response_streaming("system prompt", "user message", &[], tx)
            .await;

        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
        assert_eq!(result.unwrap(), "Hello world");

        // Verify each chunk arrived through the channel
        let mut chunks = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            chunks.push(chunk);
        }
        assert_eq!(chunks, vec!["Hello", " world"]);

        server.verify().await;
    }

    #[tokio::test]
    async fn test_generate_response_streaming_returns_err_on_api_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(401).set_body_string(
                r#"{"error":{"type":"authentication_error","message":"Invalid API key"}}"#,
            ))
            .mount(&server)
            .await;

        let config = ClaudeConfig {
            api_key: "bad-key".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 100,
            temperature: 1.0,
            base_url: server.uri(),
        };
        let client = ClaudeClient::new(config);

        let (tx, _rx) = mpsc::channel::<String>(16);
        let result = client
            .generate_response_streaming("system", "user message", &[], tx)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("401"),
            "Error message should mention 401, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_generate_response_streaming_with_conversation_history() {
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(body_string_contains("previous user message"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(
                        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"response\"}}\ndata: [DONE]\n",
                    ),
            )
            .expect(1)
            .mount(&server)
            .await;

        let config = ClaudeConfig {
            api_key: "test-key".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 100,
            temperature: 1.0,
            base_url: server.uri(),
        };
        let client = ClaudeClient::new(config);

        let history = vec![
            Message {
                role: "user".to_string(),
                content: "previous user message".to_string(),
                message_id: Some(1),
            },
            Message {
                role: "assistant".to_string(),
                content: "previous reply".to_string(),
                message_id: None,
            },
        ];

        let (tx, _rx) = mpsc::channel::<String>(16);
        let result = client
            .generate_response_streaming("system", "follow-up", &history, tx)
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "response");
        server.verify().await;
    }
}
