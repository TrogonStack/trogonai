use std::collections::VecDeque;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream::{LocalBoxStream, StreamExt as _};
use futures_util::{Stream, stream};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

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
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let mut attempt = 0u32;
        loop {
            let response = tokio::time::timeout(
                self.request_timeout,
                self.http
                    .post(&url)
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

            if response.status().is_success() {
                return Ok(response);
            }

            let status = response.status();
            let retryable = matches!(status.as_u16(), 429 | 503) && attempt < MAX_RETRIES;
            let delay = retry_delay(response.headers(), attempt);
            let text = response.text().await.unwrap_or_default();
            warn!(status = %status, body = %text, attempt, "openrouter: API error response");

            if retryable {
                attempt += 1;
                info!(
                    status = %status,
                    delay_ms = delay.as_millis(),
                    attempt,
                    "openrouter: retrying after transient error"
                );
                tokio::time::sleep(delay).await;
            } else {
                return Err(format!("OpenRouter API error {status}: {text}"));
            }
        }
    }
}

const MAX_RETRIES: u32 = 3;

fn retry_delay(headers: &reqwest::header::HeaderMap, attempt: u32) -> Duration {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(1u64 << attempt.min(5)))
        .min(Duration::from_secs(30))
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

    // ── Message builders ──────────────────────────────────────────────────────

    #[test]
    fn message_user_sets_role_and_content() {
        let m = Message::user("hello");
        assert_eq!(m.role, "user");
        assert_eq!(m.content, "hello");
        assert!(m.prompt_tokens.is_none());
        assert!(m.completion_tokens.is_none());
    }

    #[test]
    fn message_assistant_sets_role_and_content() {
        let m = Message::assistant("reply");
        assert_eq!(m.role, "assistant");
        assert_eq!(m.content, "reply");
        assert!(m.prompt_tokens.is_none());
        assert!(m.completion_tokens.is_none());
    }

    #[test]
    fn message_system_sets_role_and_content() {
        let m = Message::system("be concise");
        assert_eq!(m.role, "system");
        assert_eq!(m.content, "be concise");
        assert!(m.prompt_tokens.is_none());
        assert!(m.completion_tokens.is_none());
    }

    #[test]
    fn message_assistant_with_usage_stores_token_counts() {
        let m = Message::assistant_with_usage("reply", 100, 50);
        assert_eq!(m.role, "assistant");
        assert_eq!(m.content, "reply");
        assert_eq!(m.prompt_tokens, Some(100));
        assert_eq!(m.completion_tokens, Some(50));
    }

    #[test]
    fn message_serialization_omits_none_token_fields() {
        let m = Message::user("hi");
        let v = serde_json::to_value(&m).unwrap();
        assert!(v.get("prompt_tokens").is_none());
        assert!(v.get("completion_tokens").is_none());
    }

    #[test]
    fn message_with_usage_serializes_token_fields() {
        let m = Message::assistant_with_usage("ok", 10, 5);
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["prompt_tokens"], 10);
        assert_eq!(v["completion_tokens"], 5);
    }

    // ── FinishReason ──────────────────────────────────────────────────────────

    #[test]
    fn finish_reason_stop() {
        assert_eq!(FinishReason::from_str("stop"), FinishReason::Stop);
    }

    #[test]
    fn finish_reason_length() {
        assert_eq!(FinishReason::from_str("length"), FinishReason::Length);
    }

    #[test]
    fn finish_reason_unknown_becomes_other() {
        assert_eq!(
            FinishReason::from_str("content_filter"),
            FinishReason::Other("content_filter".to_string())
        );
    }

    #[test]
    fn finish_reason_empty_string_becomes_other() {
        assert_eq!(
            FinishReason::from_str(""),
            FinishReason::Other("".to_string())
        );
    }

    // ── SSE parser edge cases ─────────────────────────────────────────────────

    #[test]
    fn skips_empty_content_delta() {
        let events = events_from_lines(&[
            r#"data: {"choices":[{"delta":{"content":""},"finish_reason":null}]}"#,
        ]);
        assert!(events.is_empty(), "empty content string must not emit TextDelta");
    }

    #[test]
    fn skips_malformed_json() {
        let events = events_from_lines(&["data: {not valid json"]);
        assert!(events.is_empty(), "malformed JSON must be silently skipped");
    }

    #[test]
    fn skips_data_line_with_no_choices_and_no_usage() {
        let events = events_from_lines(&[r#"data: {"id":"chatcmpl-xyz","object":"chat.completion.chunk"}"#]);
        assert!(events.is_empty());
    }

    #[test]
    fn parse_finish_reason_length() {
        let events = events_from_lines(&[
            r#"data: {"choices":[{"delta":{},"finish_reason":"length"}]}"#,
        ]);
        assert!(
            matches!(&events[0], OpenRouterEvent::Finished { reason } if *reason == FinishReason::Length)
        );
    }

    #[test]
    fn parse_finish_reason_unknown() {
        let events = events_from_lines(&[
            r#"data: {"choices":[{"delta":{},"finish_reason":"content_filter"}]}"#,
        ]);
        assert!(
            matches!(&events[0], OpenRouterEvent::Finished { reason } if *reason == FinishReason::Other("content_filter".to_string()))
        );
    }

    #[test]
    fn usage_and_finish_in_same_chunk_emits_finished_then_usage() {
        let events = events_from_lines(&[
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}}"#,
        ]);
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], OpenRouterEvent::Finished { reason: FinishReason::Stop }));
        assert!(matches!(&events[1], OpenRouterEvent::Usage { prompt_tokens: 3, completion_tokens: 1 }));
    }

    #[test]
    fn usage_zero_completion_tokens_is_allowed() {
        let events = events_from_lines(&[
            r#"data: {"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":0,"total_tokens":7}}"#,
        ]);
        assert!(
            matches!(&events[0], OpenRouterEvent::Usage { prompt_tokens: 7, completion_tokens: 0 })
        );
    }

    #[tokio::test]
    async fn stream_split_across_chunks_reassembles_line() {
        use futures_util::stream;

        // SSE line split in the middle of the JSON across two chunks.
        let chunks: Vec<Result<Bytes, reqwest::Error>> = vec![
            Ok(Bytes::from("data: {\"choices\":[{\"delta\":{\"con")),
            Ok(Bytes::from("tent\":\"A\"},\"finish_reason\":null}]}\n")),
            Ok(Bytes::from("data: [DONE]\n")),
        ];

        let events: Vec<_> = parse_sse(stream::iter(chunks)).collect().await;
        assert!(matches!(&events[0], OpenRouterEvent::TextDelta { text } if text == "A"));
        assert!(matches!(&events[1], OpenRouterEvent::Done));
    }

    #[tokio::test]
    async fn stream_crlf_line_endings_are_handled() {
        use futures_util::stream;

        let chunks: Vec<Result<Bytes, reqwest::Error>> = vec![
            Ok(Bytes::from(
                "data: {\"choices\":[{\"delta\":{\"content\":\"B\"},\"finish_reason\":null}]}\r\n",
            )),
            Ok(Bytes::from("data: [DONE]\r\n")),
        ];

        let events: Vec<_> = parse_sse(stream::iter(chunks)).collect().await;
        assert!(matches!(&events[0], OpenRouterEvent::TextDelta { text } if text == "B"));
        assert!(matches!(&events[1], OpenRouterEvent::Done));
    }

    // ── retry_delay ───────────────────────────────────────────────────────────

    fn empty_headers() -> reqwest::header::HeaderMap {
        reqwest::header::HeaderMap::new()
    }

    fn headers_with_retry_after(secs: &str) -> reqwest::header::HeaderMap {
        let mut m = reqwest::header::HeaderMap::new();
        m.insert("retry-after", secs.parse().unwrap());
        m
    }

    #[test]
    fn retry_delay_no_header_uses_exponential_backoff() {
        assert_eq!(retry_delay(&empty_headers(), 0), Duration::from_secs(1));
        assert_eq!(retry_delay(&empty_headers(), 1), Duration::from_secs(2));
        assert_eq!(retry_delay(&empty_headers(), 2), Duration::from_secs(4));
        assert_eq!(retry_delay(&empty_headers(), 3), Duration::from_secs(8));
    }

    #[test]
    fn retry_delay_header_overrides_backoff() {
        assert_eq!(retry_delay(&headers_with_retry_after("5"), 0), Duration::from_secs(5));
        assert_eq!(retry_delay(&headers_with_retry_after("0"), 2), Duration::from_secs(0));
    }

    #[test]
    fn retry_delay_is_capped_at_30s() {
        assert_eq!(retry_delay(&empty_headers(), 5), Duration::from_secs(30));
        assert_eq!(retry_delay(&headers_with_retry_after("60"), 0), Duration::from_secs(30));
    }

    #[test]
    fn retry_delay_ignores_non_numeric_retry_after() {
        // HTTP-date format in Retry-After is not supported; falls back to backoff.
        let h = headers_with_retry_after("Wed, 21 Oct 2025 07:28:00 GMT");
        assert_eq!(retry_delay(&h, 0), Duration::from_secs(1));
    }

    #[test]
    fn retry_delay_at_attempt_4_is_below_cap() {
        // 2^4 = 16, which is under the 30s cap.
        assert_eq!(retry_delay(&empty_headers(), 4), Duration::from_secs(16));
    }

    // ── SSE parser edge cases ─────────────────────────────────────────────────

    #[test]
    fn parse_content_null_is_skipped() {
        let events = events_from_lines(&[
            r#"data: {"choices":[{"delta":{"content":null},"finish_reason":null}]}"#,
        ]);
        assert!(events.is_empty(), "null content must not emit TextDelta");
    }

    #[test]
    fn parse_finish_reason_null_is_skipped() {
        // finish_reason: null means still generating — must not emit a Finished event.
        let events = events_from_lines(&[
            r#"data: {"choices":[{"delta":{"content":"text"},"finish_reason":null}]}"#,
        ]);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], OpenRouterEvent::TextDelta { .. }));
    }

    #[test]
    fn parse_usage_empty_object_is_skipped() {
        // Empty usage object must not emit a Usage event.
        let events = events_from_lines(&[
            r#"data: {"choices":[],"usage":{}}"#,
        ]);
        assert!(events.is_empty(), "empty usage object must not emit Usage event");
    }

    #[test]
    fn parse_usage_missing_completion_tokens_defaults_to_zero() {
        let events = events_from_lines(&[
            r#"data: {"choices":[],"usage":{"prompt_tokens":10}}"#,
        ]);
        assert!(
            matches!(&events[0], OpenRouterEvent::Usage { prompt_tokens: 10, completion_tokens: 0 }),
            "missing completion_tokens must default to 0: {events:?}"
        );
    }

    #[test]
    fn parse_usage_missing_prompt_tokens_defaults_to_zero() {
        let events = events_from_lines(&[
            r#"data: {"choices":[],"usage":{"completion_tokens":5}}"#,
        ]);
        assert!(
            matches!(&events[0], OpenRouterEvent::Usage { prompt_tokens: 0, completion_tokens: 5 }),
            "missing prompt_tokens must default to 0: {events:?}"
        );
    }

    #[test]
    fn parse_text_with_unicode_characters() {
        let events = events_from_lines(&[
            r#"data: {"choices":[{"delta":{"content":"こんにちは 🌸"},"finish_reason":null}]}"#,
        ]);
        assert!(
            matches!(&events[0], OpenRouterEvent::TextDelta { text } if text == "こんにちは 🌸")
        );
    }

    #[test]
    fn parse_multiple_choices_uses_first() {
        // When multiple choices are present (e.g., n>1), only the first is used.
        let events = events_from_lines(&[
            r#"data: {"choices":[{"delta":{"content":"first"},"finish_reason":null},{"delta":{"content":"second"},"finish_reason":null}]}"#,
        ]);
        assert_eq!(events.len(), 1, "only first choice must be used");
        assert!(matches!(&events[0], OpenRouterEvent::TextDelta { text } if text == "first"));
    }

    // ── Message edge cases ────────────────────────────────────────────────────

    #[test]
    fn message_empty_content_has_correct_role() {
        let m = Message::user("");
        assert_eq!(m.role, "user");
        assert_eq!(m.content, "");
    }

    #[test]
    fn message_empty_content_serializes_correctly() {
        let m = Message::user("");
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["role"], "user");
        assert_eq!(v["content"], "");
        assert!(v.get("prompt_tokens").is_none());
    }

    #[test]
    fn message_clone_is_independent() {
        let m = Message::assistant_with_usage("hello", 10, 5);
        let m2 = m.clone();
        assert_eq!(m2.content, "hello");
        assert_eq!(m2.prompt_tokens, Some(10));
        assert_eq!(m2.completion_tokens, Some(5));
    }
}
