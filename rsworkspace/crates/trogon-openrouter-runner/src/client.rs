use std::collections::VecDeque;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream::{LocalBoxStream, StreamExt as _};
use futures_util::{Stream, stream};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::http_client::OpenRouterHttpClient;

/// A message in the OpenAI-compatible conversation format.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: text.into(),
            prompt_tokens: None,
            completion_tokens: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: text.into(),
            prompt_tokens: None,
            completion_tokens: None,
        }
    }

    pub fn assistant_with_usage(
        text: impl Into<String>,
        prompt_tokens: u64,
        completion_tokens: u64,
    ) -> Self {
        Self {
            role: "assistant".to_string(),
            content: text.into(),
            prompt_tokens: Some(prompt_tokens),
            completion_tokens: Some(completion_tokens),
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: text.into(),
            prompt_tokens: None,
            completion_tokens: None,
        }
    }
}

/// Why the model stopped generating.
#[derive(Debug, Clone, PartialEq)]
pub enum FinishReason {
    Stop,
    Length,
    Other(String),
}

impl FinishReason {
    fn from_str(s: &str) -> Self {
        match s {
            "stop" => Self::Stop,
            "length" => Self::Length,
            other => Self::Other(other.to_string()),
        }
    }
}

/// An event emitted by the OpenRouter SSE stream (OpenAI chat completions format).
#[derive(Debug, Clone)]
pub enum OpenRouterEvent {
    TextDelta { text: String },
    Usage {
        prompt_tokens: u64,
        completion_tokens: u64,
    },
    Finished {
        reason: FinishReason,
    },
    Done,
    Error {
        message: String,
    },
}

/// HTTP client for OpenRouter's OpenAI-compatible chat completions API.
///
/// Does not store an API key — callers pass it per-request so sessions can use
/// individual user keys.
pub struct OpenRouterClient {
    http: reqwest::Client,
    base_url: String,
    request_timeout: Duration,
}

impl Default for OpenRouterClient {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenRouterClient {
    pub fn new() -> Self {
        let base_url = std::env::var("OPENROUTER_BASE_URL")
            .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_string());
        let request_timeout = std::env::var("OPENROUTER_PROMPT_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&n| n > 0)
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(300));
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self {
            http,
            base_url,
            request_timeout,
        }
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
            request_timeout: Duration::from_secs(300),
        }
    }

    async fn do_chat_stream(
        &self,
        model: &str,
        messages: &[Message],
        api_key: &str,
    ) -> LocalBoxStream<'static, OpenRouterEvent> {
        debug!(model, messages_len = messages.len(), "openrouter: starting chat stream");

        match self.start_request(model, messages, api_key).await {
            Ok(response) => parse_sse(response.bytes_stream()).boxed_local(),
            Err(e) => {
                warn!(error = %e, "openrouter: request failed");
                stream::once(async move { OpenRouterEvent::Error { message: e } }).boxed_local()
            }
        }
    }

    async fn start_request(
        &self,
        model: &str,
        messages: &[Message],
        api_key: &str,
    ) -> Result<reqwest::Response, String> {
        // Strip usage fields before sending — OpenRouter only accepts role + content.
        let wire_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
            .collect();

        let body = serde_json::json!({
            "model": model,
            "messages": wire_messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });

        let site_url = std::env::var("OPENROUTER_SITE_URL")
            .unwrap_or_else(|_| "https://trogonai.com".to_string());
        let site_name = std::env::var("OPENROUTER_SITE_NAME")
            .unwrap_or_else(|_| "TrogonAI".to_string());

        let response = tokio::time::timeout(
            self.request_timeout,
            self.http
                .post(format!(
                    "{}/chat/completions",
                    self.base_url.trim_end_matches('/')
                ))
                .bearer_auth(api_key)
                .header("HTTP-Referer", &site_url)
                .header("X-Title", &site_name)
                .json(&body)
                .send(),
        )
        .await
        .map_err(|_| {
            format!(
                "OpenRouter request timed out after {}s (no response headers received)",
                self.request_timeout.as_secs()
            )
        })?
        .map_err(|e| e.to_string())?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            warn!(status = %status, body = %text, "openrouter: API error response");
            return Err(format!("OpenRouter API error {status}: {text}"));
        }

        Ok(response)
    }
}

#[async_trait(?Send)]
impl OpenRouterHttpClient for OpenRouterClient {
    async fn chat_stream(
        &self,
        model: &str,
        messages: &[Message],
        api_key: &str,
    ) -> LocalBoxStream<'static, OpenRouterEvent> {
        self.do_chat_stream(model, messages, api_key).await
    }
}

// ── SSE parser (OpenAI chat completions format) ───────────────────────────────

struct SseState {
    stream: LocalBoxStream<'static, Result<Bytes, reqwest::Error>>,
    buf: String,
    pending: VecDeque<OpenRouterEvent>,
}

fn parse_sse(
    bytes: impl Stream<Item = Result<Bytes, reqwest::Error>> + 'static,
) -> impl Stream<Item = OpenRouterEvent> {
    stream::unfold(
        SseState {
            stream: bytes.boxed_local(),
            buf: String::new(),
            pending: VecDeque::new(),
        },
        |mut state| async move {
            loop {
                if let Some(ev) = state.pending.pop_front() {
                    return Some((ev, state));
                }

                if let Some(nl) = state.buf.find('\n') {
                    let line = state.buf[..nl].trim_end_matches('\r').to_string();
                    state.buf = state.buf[nl + 1..].to_string();
                    process_sse_line(&line, &mut state.pending);
                    continue;
                }

                match state.stream.next().await {
                    Some(Ok(chunk)) => {
                        state.buf.push_str(&String::from_utf8_lossy(&chunk));
                    }
                    Some(Err(e)) => {
                        state
                            .pending
                            .push_back(OpenRouterEvent::Error { message: e.to_string() });
                        return state.pending.pop_front().map(|ev| (ev, state));
                    }
                    None => {
                        // Flush any remaining line.
                        let remaining = std::mem::take(&mut state.buf);
                        let line = remaining.trim();
                        if !line.is_empty() {
                            let mut tmp = VecDeque::new();
                            process_sse_line(line, &mut tmp);
                            state.pending.extend(tmp);
                        }
                        return state.pending.pop_front().map(|ev| (ev, state));
                    }
                }
            }
        },
    )
}

fn process_sse_line(line: &str, pending: &mut VecDeque<OpenRouterEvent>) {
    let data = match line.strip_prefix("data: ") {
        Some(d) => d.trim(),
        None => return,
    };

    if data == "[DONE]" {
        pending.push_back(OpenRouterEvent::Done);
        return;
    }

    let Ok(val) = serde_json::from_str::<serde_json::Value>(data) else {
        return;
    };

    if let Some(choice) = val["choices"].get(0) {
        let delta = &choice["delta"];

        if let Some(text) = delta["content"].as_str() {
            if !text.is_empty() {
                pending.push_back(OpenRouterEvent::TextDelta {
                    text: text.to_string(),
                });
            }
        }

        if let Some(reason) = choice["finish_reason"].as_str() {
            if !reason.is_empty() {
                pending.push_back(OpenRouterEvent::Finished {
                    reason: FinishReason::from_str(reason),
                });
            }
        }
    }

    // Usage — emitted after Finished; may arrive in a separate chunk with empty choices.
    if let Some(usage) = val.get("usage").and_then(|u| u.as_object()) {
        if !usage.is_empty() {
            let prompt_tokens = usage
                .get("prompt_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let completion_tokens = usage
                .get("completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            pending.push_back(OpenRouterEvent::Usage {
                prompt_tokens,
                completion_tokens,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt as _;

    use super::*;

    fn events_from_lines(lines: &[&str]) -> Vec<OpenRouterEvent> {
        let mut pending = VecDeque::new();
        for line in lines {
            process_sse_line(line, &mut pending);
        }
        pending.into_iter().collect()
    }

    #[test]
    fn parse_text_delta() {
        let events = events_from_lines(&[
            r#"data: {"choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}"#,
        ]);
        assert!(matches!(&events[0], OpenRouterEvent::TextDelta { text } if text == "Hello"));
    }

    #[test]
    fn parse_finish_reason_stop() {
        let events = events_from_lines(&[
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        ]);
        assert!(
            matches!(&events[0], OpenRouterEvent::Finished { reason } if *reason == FinishReason::Stop)
        );
    }

    #[test]
    fn parse_usage_chunk() {
        let events = events_from_lines(&[
            r#"data: {"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#,
        ]);
        assert!(
            matches!(&events[0], OpenRouterEvent::Usage { prompt_tokens: 10, completion_tokens: 5 })
        );
    }

    #[test]
    fn parse_done_sentinel() {
        let events = events_from_lines(&["data: [DONE]"]);
        assert!(matches!(&events[0], OpenRouterEvent::Done));
    }

    #[test]
    fn skips_non_data_lines() {
        let events = events_from_lines(&[": keep-alive", "event: message"]);
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn full_stream_sequence() {
        use futures_util::stream;

        let chunks: Vec<Result<Bytes, reqwest::Error>> = vec![
            Ok(Bytes::from(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"},\"finish_reason\":null}]}\n",
            )),
            Ok(Bytes::from(
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2,\"total_tokens\":7}}\n",
            )),
            Ok(Bytes::from("data: [DONE]\n")),
        ];

        let events: Vec<_> = parse_sse(stream::iter(chunks)).collect().await;

        assert!(matches!(&events[0], OpenRouterEvent::TextDelta { text } if text == "Hi"));
        assert!(matches!(&events[1], OpenRouterEvent::Finished { reason: FinishReason::Stop }));
        assert!(matches!(&events[2], OpenRouterEvent::Usage { prompt_tokens: 5, completion_tokens: 2 }));
        assert!(matches!(&events[3], OpenRouterEvent::Done));
    }
}
