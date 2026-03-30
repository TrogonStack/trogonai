//! HTTP client for xAI's OpenAI-compatible REST API.
//!
//! ## REST vs gRPC — decision record
//!
//! xAI exposes two API surfaces:
//! - **OpenAI-compatible REST** at `api.x.ai/v1/chat/completions`
//! - **Native gRPC** defined in [`xai-org/xai-proto`](https://github.com/xai-org/xai-proto)
//!
//! This crate uses the REST endpoint for the following reasons:
//!
//! 1. **Feature parity**: REST and gRPC both support streaming chat, tool/function
//!    calling, vision, and reasoning. REST additionally exposes code interpreter and
//!    file search, which are not in the current gRPC surface.
//!
//! 2. **No Rust gRPC SDK**: using the proto requires `tonic` + `prost` codegen via
//!    `build.rs`, adding build complexity and an unofficial crate dependency for no
//!    functional gain in the current chat-agent use case.
//!
//! 3. **Simplicity**: the OpenAI-compatible endpoint keeps the client minimal and
//!    portable across OpenAI-compatible providers.
//!
//! Revisit if xAI-exclusive features become necessary (deferred completions,
//! fine-grained `search_parameters`, or the native batch API).

use std::collections::{HashMap, VecDeque};

use bytes::Bytes;
use futures_util::stream::StreamExt as _;
use futures_util::{Stream, stream};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// A single message in the conversation history.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Message {
    pub role: String,
    pub content: String,
}

/// An event emitted by the streaming chat completions endpoint.
#[derive(Debug, Clone)]
pub enum XaiEvent {
    /// A text chunk from the model.
    TextDelta { text: String },
    /// The model requested a tool call (accumulated from `delta.tool_calls`
    /// deltas; emitted when `finish_reason` is `"tool_calls"`).
    ToolCallStart { id: String, name: String, arguments: String },
    /// Token usage reported in the final usage chunk
    /// (`stream_options: {include_usage: true}`).
    Usage { prompt_tokens: u64, completion_tokens: u64 },
    /// The stream ended normally (`[DONE]`).
    Done,
    /// A network or API error.
    Error { message: String },
}

/// HTTP client for xAI's OpenAI-compatible chat completions API.
///
/// Does not store an API key — callers pass it per-request so sessions can use
/// individual user keys.
pub struct XaiClient {
    http: reqwest::Client,
    base_url: String,
}

impl XaiClient {
    pub fn new() -> Self {
        let base_url = std::env::var("XAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.x.ai/v1".to_string());
        Self::with_base_url(base_url)
    }

    /// Construct with an explicit base URL. Useful for tests and custom proxies.
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
        }
    }

    /// Start a streaming chat completion and return a stream of `XaiEvent`s.
    ///
    /// `api_key` is the xAI bearer token to use for this specific request.
    ///
    /// `search_mode` controls xAI server-side web/X search:
    /// - `None` or `Some("off")` — no search (default)
    /// - `Some("auto")` — model decides when to search
    /// - `Some("on")` — always search
    pub async fn chat_stream(
        &self,
        model: &str,
        messages: &[Message],
        api_key: &str,
        search_mode: Option<&str>,
    ) -> impl Stream<Item = XaiEvent> {
        debug!(model, messages_len = messages.len(), search_mode, "xai: starting chat stream");

        let result = self.start_request(model, messages, api_key, search_mode).await;
        match result {
            Ok(response) => parse_sse(response.bytes_stream()).boxed_local(),
            Err(e) => {
                let msg = e.to_string();
                warn!(error = %msg, "xai: request failed");
                stream::once(async move { XaiEvent::Error { message: msg } }).boxed_local()
            }
        }
    }

    async fn start_request(
        &self,
        model: &str,
        messages: &[Message],
        api_key: &str,
        search_mode: Option<&str>,
    ) -> Result<reqwest::Response, String> {
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true }
        });

        // Inject search_parameters when mode is "auto" or "on".
        // Absent / "off" means no search — omit the field entirely so older
        // API versions that don't know the parameter are not affected.
        if let Some(mode) = search_mode.filter(|m| *m != "off") {
            body["search_parameters"] = serde_json::json!({ "mode": mode });
        }

        let response = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            warn!(status = %status, body = %text, "xai: API error response");
            return Err(format!("xAI API error {status}: {text}"));
        }

        Ok(response)
    }
}

// ── Stateful SSE parser ───────────────────────────────────────────────────────

/// Accumulated state for a single tool call while streaming deltas.
#[derive(Default)]
struct ToolCallAcc {
    id: String,
    name: String,
    arguments: String,
}

struct SseState {
    stream: futures_util::stream::LocalBoxStream<'static, Result<Bytes, reqwest::Error>>,
    buf: String,
    /// Tool call fragments indexed by `delta.tool_calls[].index`.
    tool_calls: HashMap<usize, ToolCallAcc>,
    /// Events ready to be yielded before pulling more bytes.
    pending: VecDeque<XaiEvent>,
}

/// Parse a raw SSE byte stream into `XaiEvent`s.
///
/// Maintains state across chunks to:
/// - accumulate partial lines split across TCP segments
/// - accumulate `delta.tool_calls` fragments until `finish_reason: "tool_calls"`
/// - capture the trailing usage chunk emitted by `stream_options.include_usage`
fn parse_sse(
    bytes: impl Stream<Item = Result<Bytes, reqwest::Error>> + 'static,
) -> impl Stream<Item = XaiEvent> {
    stream::unfold(
        SseState {
            stream: bytes.boxed_local(),
            buf: String::new(),
            tool_calls: HashMap::new(),
            pending: VecDeque::new(),
        },
        |mut state| async move {
            loop {
                // Yield any buffered events before reading more bytes.
                if let Some(ev) = state.pending.pop_front() {
                    return Some((ev, state));
                }

                // Consume one complete line from the text buffer.
                if let Some(nl) = state.buf.find('\n') {
                    let line = state.buf[..nl].trim_end_matches('\r').to_string();
                    state.buf = state.buf[nl + 1..].to_string();
                    process_sse_line(&line, &mut state.tool_calls, &mut state.pending);
                    continue;
                }

                // Need more bytes from the network.
                match state.stream.next().await {
                    Some(Ok(chunk)) => {
                        state.buf.push_str(&String::from_utf8_lossy(&chunk));
                    }
                    Some(Err(e)) => {
                        state.pending.push_back(XaiEvent::Error { message: e.to_string() });
                        return state.pending.pop_front().map(|ev| (ev, state));
                    }
                    None => {
                        // Flush any remaining unterminated line.
                        let remaining = std::mem::take(&mut state.buf);
                        let line = remaining.trim();
                        if !line.is_empty() {
                            process_sse_line(line, &mut state.tool_calls, &mut state.pending);
                        }
                        return state.pending.pop_front().map(|ev| (ev, state));
                    }
                }
            }
        },
    )
}

/// Process one `data: ...` SSE line, pushing resulting events into `pending`.
///
/// Tool call deltas are accumulated in `tool_calls`; the completed tool calls
/// are flushed to `pending` when `finish_reason` is `"tool_calls"`.
fn process_sse_line(
    line: &str,
    tool_calls: &mut HashMap<usize, ToolCallAcc>,
    pending: &mut VecDeque<XaiEvent>,
) {
    let data = match line.strip_prefix("data: ") {
        Some(d) => d,
        None => return,
    };

    if data == "[DONE]" {
        pending.push_back(XaiEvent::Done);
        return;
    }

    let val: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, data, "xai: failed to parse SSE data line");
            return;
        }
    };

    // Usage chunk: `{"choices": [], "usage": {...}}` — emitted last when
    // `stream_options.include_usage` is true.
    if val["choices"].as_array().map(|c| c.is_empty()).unwrap_or(false) {
        let p = val["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
        let c = val["usage"]["completion_tokens"].as_u64().unwrap_or(0);
        if p > 0 || c > 0 {
            pending.push_back(XaiEvent::Usage { prompt_tokens: p, completion_tokens: c });
        }
        return;
    }

    let choice = &val["choices"][0];
    let delta = &choice["delta"];
    let finish_reason = choice["finish_reason"].as_str();

    // Text content delta.
    if let Some(text) = delta["content"].as_str() {
        if !text.is_empty() {
            pending.push_back(XaiEvent::TextDelta { text: text.to_string() });
        }
    }

    // Accumulate tool call fragments.
    if let Some(tc_arr) = delta["tool_calls"].as_array() {
        for tc in tc_arr {
            let index = tc["index"].as_u64().unwrap_or(0) as usize;
            let acc = tool_calls.entry(index).or_default();
            if let Some(id) = tc["id"].as_str() {
                acc.id = id.to_string();
            }
            if let Some(name) = tc["function"]["name"].as_str() {
                acc.name.push_str(name);
            }
            if let Some(args) = tc["function"]["arguments"].as_str() {
                acc.arguments.push_str(args);
            }
        }
    }

    // Flush accumulated tool calls when the model signals it's done calling tools.
    if finish_reason == Some("tool_calls") {
        let mut sorted: Vec<_> = tool_calls.drain().collect();
        sorted.sort_by_key(|(idx, _)| *idx);
        for (_, acc) in sorted {
            pending.push_back(XaiEvent::ToolCallStart {
                id: acc.id,
                name: acc.name,
                arguments: acc.arguments,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: run `process_sse_line` with fresh state and return the
    /// first pending event (ignores any extras for simplicity).
    fn parse_line(line: &str) -> Option<XaiEvent> {
        let mut tool_calls = HashMap::new();
        let mut pending = VecDeque::new();
        process_sse_line(line, &mut tool_calls, &mut pending);
        pending.pop_front()
    }

    fn chunk(text: &str) -> String {
        format!(
            r#"data: {{"choices":[{{"delta":{{"content":"{text}"}}}}]}}"#
        )
    }

    #[test]
    fn text_delta() {
        let line = chunk("hello");
        let event = parse_line(&line).unwrap();
        assert!(matches!(event, XaiEvent::TextDelta { text } if text == "hello"));
    }

    #[test]
    fn done_signal() {
        let event = parse_line("data: [DONE]").unwrap();
        assert!(matches!(event, XaiEvent::Done));
    }

    #[test]
    fn non_data_prefix_returns_none() {
        assert!(parse_line("event: message").is_none());
        assert!(parse_line(": keep-alive").is_none());
        assert!(parse_line("").is_none());
    }

    #[test]
    fn empty_content_returns_none() {
        let line = r#"data: {"choices":[{"delta":{"content":""}}]}"#;
        assert!(parse_line(line).is_none());
    }

    #[test]
    fn missing_content_field_returns_none() {
        let line = r#"data: {"choices":[{"delta":{}}]}"#;
        assert!(parse_line(line).is_none());
    }

    #[test]
    fn invalid_json_returns_none() {
        assert!(parse_line("data: {not valid json}").is_none());
    }

    #[test]
    fn role_only_delta_returns_none() {
        let line = r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#;
        assert!(parse_line(line).is_none());
    }

    #[test]
    fn usage_chunk_emits_usage_event() {
        let line = r#"data: {"choices":[],"usage":{"prompt_tokens":42,"completion_tokens":7,"total_tokens":49}}"#;
        let event = parse_line(line).unwrap();
        assert!(
            matches!(event, XaiEvent::Usage { prompt_tokens: 42, completion_tokens: 7 }),
            "unexpected event: {event:?}"
        );
    }

    #[test]
    fn tool_call_accumulated_on_finish_reason() {
        let mut tool_calls = HashMap::new();
        let mut pending = VecDeque::new();

        // First delta: id + function name start
        process_sse_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"web_se","arguments":""}}]},"finish_reason":null}]}"#,
            &mut tool_calls,
            &mut pending,
        );
        assert!(pending.is_empty(), "no event yet — still accumulating");

        // Second delta: name + arguments
        process_sse_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"arch","arguments":"{\"q\":"}}]},"finish_reason":null}]}"#,
            &mut tool_calls,
            &mut pending,
        );
        assert!(pending.is_empty());

        // Final delta: finish_reason = "tool_calls"
        process_sse_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"test\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            &mut tool_calls,
            &mut pending,
        );

        let event = pending.pop_front().unwrap();
        match event {
            XaiEvent::ToolCallStart { id, name, arguments } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "web_search");
                assert_eq!(arguments, r#"{"q":"test"}"#);
            }
            other => panic!("expected ToolCallStart, got {other:?}"),
        }
    }
}
