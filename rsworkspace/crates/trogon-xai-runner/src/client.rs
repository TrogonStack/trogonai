//! HTTP client for xAI's OpenAI-compatible REST API.
//!
//! ## Endpoint choice — decision record
//!
//! xAI exposes two REST surfaces and one gRPC surface:
//! - **Responses API** at `api.x.ai/v1/responses` ← used here
//! - **OpenAI-compatible** at `api.x.ai/v1/chat/completions`
//! - **Native gRPC** defined in [`xai-org/xai-proto`](https://github.com/xai-org/xai-proto)
//!
//! This crate uses the **Responses API** for the following reasons:
//!
//! 1. **Server-side tools**: `web_search`, `x_search`, `code_interpreter`, and
//!    `file_search` are only available on the Responses API.
//!    Chat Completions supports custom function calling only.
//!
//! 2. **Stateful multi-turn**: `previous_response_id` lets subsequent requests
//!    reference prior context without re-sending the full history, reducing token
//!    usage and latency.
//!
//! 3. **Agentic design**: the Responses API is purpose-built for agentic loops —
//!    the model can call tools, receive results, and continue in one logical turn.
//!
//! 4. **No Rust gRPC SDK**: gRPC requires `tonic` + `prost` codegen with no
//!    functional gain for the current chat-agent use case.
//!
//! Revisit gRPC only if xAI-exclusive features (Batch API, `agent_count`,
//! `use_encrypted_content`) become necessary.

use std::collections::VecDeque;

use bytes::Bytes;
use futures_util::stream::StreamExt as _;
use futures_util::{Stream, stream};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// A function called by the model within a tool call message.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

/// A tool call entry within an `assistant` message's `tool_calls` array.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MessageToolCall {
    pub id: String,
    /// Always `"function"` for xAI function calls.
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolCallFunction,
}

/// A single message in the conversation history.
///
/// Supports all xAI message roles:
/// - `user` / `system` — always have `content`
/// - `assistant` — has `content` (text) or `tool_calls` (function calls) or both
/// - `tool` — has `content` and `tool_call_id` (result of a function call)
///
/// All fields except `role` are `Option` so that:
/// 1. Messages without a given field serialize without a `null` placeholder
///    (`skip_serializing_if = "Option::is_none"`).
/// 2. Legacy sessions stored as `{"role":"user","content":"hello"}` deserialize
///    correctly — `#[serde(default)]` fills missing fields with `None`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Message {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<MessageToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self { role: "user".to_string(), content: Some(text.into()), tool_calls: None, tool_call_id: None }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self { role: "system".to_string(), content: Some(text.into()), tool_calls: None, tool_call_id: None }
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self { role: "assistant".to_string(), content: Some(text.into()), tool_calls: None, tool_call_id: None }
    }

    pub fn assistant_tool_calls(tool_calls: Vec<MessageToolCall>) -> Self {
        Self { role: "assistant".to_string(), content: None, tool_calls: Some(tool_calls), tool_call_id: None }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self { role: "tool".to_string(), content: Some(content.into()), tool_calls: None, tool_call_id: Some(tool_call_id.into()) }
    }

    /// Returns the text content of this message, or `""` if none.
    pub fn content_str(&self) -> &str {
        self.content.as_deref().unwrap_or("")
    }
}

/// An item in the `input` array sent to the Responses API.
///
/// Three variants map to the three kinds of items the Responses API accepts:
/// - `Message` — user / system / assistant text turns
/// - `FunctionCall` — assistant tool call recorded from a previous response
/// - `FunctionCallOutput` — result of executing a function call
#[derive(Serialize, Clone, Debug)]
#[serde(untagged)]
pub enum InputItem {
    Message {
        role: String,
        content: String,
    },
    FunctionCall {
        #[serde(rename = "type")]
        kind: String,
        call_id: String,
        name: String,
        arguments: String,
    },
    FunctionCallOutput {
        #[serde(rename = "type")]
        kind: String,
        call_id: String,
        output: String,
    },
}

impl InputItem {
    pub fn user(content: impl Into<String>) -> Self {
        Self::Message { role: "user".to_string(), content: content.into() }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::Message { role: "system".to_string(), content: content.into() }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Message { role: "assistant".to_string(), content: content.into() }
    }

    pub fn function_call(
        call_id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self::FunctionCall {
            kind: "function_call".to_string(),
            call_id: call_id.into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    pub fn function_call_output(call_id: impl Into<String>, output: impl Into<String>) -> Self {
        Self::FunctionCallOutput {
            kind: "function_call_output".to_string(),
            call_id: call_id.into(),
            output: output.into(),
        }
    }
}

/// Why the model stopped generating.
///
/// Mapped from the Responses API `response.status` field in `response.completed`.
#[derive(Debug, Clone, PartialEq)]
pub enum FinishReason {
    /// Normal end of turn — response is complete.
    Completed,
    /// Generation stopped before completion (e.g. max_output_tokens reached).
    Incomplete,
    /// Generation failed with an error.
    Failed,
    /// Generation was cancelled.
    Cancelled,
    /// Unknown / other status.
    Other(String),
}

impl FinishReason {
    fn from_status(s: &str) -> Self {
        match s {
            "completed" => Self::Completed,
            "incomplete" => Self::Incomplete,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            other => Self::Other(other.to_string()),
        }
    }
}

/// An event emitted by the Responses API streaming endpoint.
#[derive(Debug, Clone)]
pub enum XaiEvent {
    /// A text chunk from the model.
    TextDelta { text: String },
    /// The model requested a function call.
    ///
    /// In the Responses API, function calls are delivered as a complete chunk
    /// (not streamed in deltas like Chat Completions).
    FunctionCall { call_id: String, name: String, arguments: String },
    /// The `id` of this response — used as `previous_response_id` next turn.
    ResponseId { id: String },
    /// Token usage from the `response.completed` event.
    Usage { prompt_tokens: u64, completion_tokens: u64 },
    /// Why the model stopped — included in the `response.completed` event.
    Finished { reason: FinishReason },
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

    /// Start a streaming Responses API call and return a stream of `XaiEvent`s.
    ///
    /// - `input` — items for this turn (user message, or tool results for follow-up turns)
    /// - `api_key` — xAI bearer token for this request
    /// - `tools` — server-side tool names to enable, e.g. `["web_search", "x_search"]`
    /// - `previous_response_id` — ID from the prior response; enables stateful
    ///   multi-turn without re-sending full history
    /// - `max_turns` — maximum agentic tool-call iterations the server may perform
    pub async fn chat_stream(
        &self,
        model: &str,
        input: &[InputItem],
        api_key: &str,
        tools: &[String],
        previous_response_id: Option<&str>,
        max_turns: Option<u32>,
    ) -> impl Stream<Item = XaiEvent> + use<> {
        debug!(
            model,
            input_len = input.len(),
            tools = ?tools,
            has_prev_response = previous_response_id.is_some(),
            "xai: starting responses stream"
        );

        let result = self.start_request(model, input, api_key, tools, previous_response_id, max_turns).await;
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
        input: &[InputItem],
        api_key: &str,
        tools: &[String],
        previous_response_id: Option<&str>,
        max_turns: Option<u32>,
    ) -> Result<reqwest::Response, String> {
        let mut body = serde_json::json!({
            "model": model,
            "input": input,
            "stream": true,
        });

        if !tools.is_empty() {
            let tools_json: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| serde_json::json!({ "type": t }))
                .collect();
            body["tools"] = serde_json::Value::Array(tools_json);
        }

        if let Some(prev_id) = previous_response_id {
            body["previous_response_id"] = serde_json::Value::String(prev_id.to_string());
        }

        if let Some(turns) = max_turns {
            body["max_turns"] = serde_json::Value::Number(turns.into());
        }

        let response = self
            .http
            .post(format!("{}/responses", self.base_url.trim_end_matches('/')))
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

// ── Stateful SSE parser (Responses API) ──────────────────────────────────────

struct SseState {
    stream: futures_util::stream::LocalBoxStream<'static, Result<Bytes, reqwest::Error>>,
    buf: String,
    /// Set to true once a `ResponseId` event has been emitted for this stream.
    response_id_emitted: bool,
    /// Events ready to be yielded before pulling more bytes.
    pending: VecDeque<XaiEvent>,
}

/// Parse a raw SSE byte stream from the Responses API into `XaiEvent`s.
fn parse_sse(
    bytes: impl Stream<Item = Result<Bytes, reqwest::Error>> + 'static,
) -> impl Stream<Item = XaiEvent> {
    stream::unfold(
        SseState {
            stream: bytes.boxed_local(),
            buf: String::new(),
            response_id_emitted: false,
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
                    process_sse_line(&line, &mut state.response_id_emitted, &mut state.pending);
                    continue;
                }

                match state.stream.next().await {
                    Some(Ok(chunk)) => {
                        state.buf.push_str(&String::from_utf8_lossy(&chunk));
                    }
                    Some(Err(e)) => {
                        state.pending.push_back(XaiEvent::Error { message: e.to_string() });
                        return state.pending.pop_front().map(|ev| (ev, state));
                    }
                    None => {
                        let remaining = std::mem::take(&mut state.buf);
                        let line = remaining.trim();
                        if !line.is_empty() {
                            process_sse_line(line, &mut state.response_id_emitted, &mut state.pending);
                        }
                        return state.pending.pop_front().map(|ev| (ev, state));
                    }
                }
            }
        },
    )
}

/// Process one `data: ...` SSE line from the Responses API.
///
/// Responses API event types handled:
/// - `message.delta` / `response.output_text.delta` → `TextDelta`
///   Both names are accepted: `message.delta` is the documented xAI name;
///   `response.output_text.delta` is the OpenAI Responses API spec name.
///   The `delta` field may be an object `{"text":"..."}` or a bare string.
/// - `function_call` → `FunctionCall` (complete, not streamed in fragments)
/// - `response.completed` / `response.done` → `Usage` + `Finished`
/// - `[DONE]` → `Done`
///
/// The top-level `id` field present on most events is used to emit `ResponseId`
/// exactly once per stream (the first time it appears).
fn process_sse_line(
    line: &str,
    response_id_emitted: &mut bool,
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

    // Emit the response ID once (the first chunk that carries it).
    if !*response_id_emitted {
        if let Some(id) = val["id"].as_str() {
            pending.push_back(XaiEvent::ResponseId { id: id.to_string() });
            *response_id_emitted = true;
        }
    }

    let event_type = val["type"].as_str().unwrap_or("");

    match event_type {
        // Accept both documented xAI name ("message.delta") and OpenAI spec name
        // ("response.output_text.delta"). The delta payload may be an object with
        // a "text" field or a bare string — try both.
        "message.delta" | "response.output_text.delta" => {
            if let Some(text) = val["delta"]["text"].as_str()
                .or_else(|| val["delta"].as_str())
            {
                if !text.is_empty() {
                    pending.push_back(XaiEvent::TextDelta { text: text.to_string() });
                }
            }
        }
        "function_call" => {
            // {"type":"function_call","function_call":{"call_id":"...","name":"...","arguments":"..."}}
            let fc = &val["function_call"];
            let call_id = fc["call_id"].as_str().unwrap_or("").to_string();
            let name = fc["name"].as_str().unwrap_or("").to_string();
            let arguments = fc["arguments"].as_str().unwrap_or("").to_string();
            if !call_id.is_empty() || !name.is_empty() {
                pending.push_back(XaiEvent::FunctionCall { call_id, name, arguments });
            }
        }
        "response.completed" | "response.done" => {
            // Usage may be top-level (xAI extension) or nested inside the
            // response object (per OpenAI Responses API spec).
            let usage = if val["usage"].is_null() { &val["response"]["usage"] } else { &val["usage"] };
            let p = usage["prompt_tokens"].as_u64().unwrap_or(0);
            let c = usage["completion_tokens"].as_u64().unwrap_or(0);
            if p > 0 || c > 0 {
                pending.push_back(XaiEvent::Usage { prompt_tokens: p, completion_tokens: c });
            }
            // Emit the finish reason from response.status (Responses API field).
            // Falls back to checking top-level status for xAI-specific deviations.
            let status = val["response"]["status"].as_str()
                .or_else(|| val["status"].as_str());
            if let Some(s) = status {
                pending.push_back(XaiEvent::Finished {
                    reason: FinishReason::from_status(s),
                });
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: run `process_sse_line` with fresh state and return the
    /// first pending event (ignores any extras for simplicity).
    fn parse_line(line: &str) -> Option<XaiEvent> {
        let mut emitted = false;
        let mut pending = VecDeque::new();
        process_sse_line(line, &mut emitted, &mut pending);
        pending.pop_front()
    }

    /// Parse a line and return ALL emitted events.
    fn parse_line_all(line: &str) -> Vec<XaiEvent> {
        let mut emitted = false;
        let mut pending = VecDeque::new();
        process_sse_line(line, &mut emitted, &mut pending);
        pending.into_iter().collect()
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
    fn invalid_json_returns_none() {
        assert!(parse_line("data: {not valid json}").is_none());
    }

    #[test]
    fn text_delta_responses_api() {
        let line = r#"data: {"type":"message.delta","delta":{"type":"output_text","text":"hello"}}"#;
        let event = parse_line(line).unwrap();
        assert!(matches!(event, XaiEvent::TextDelta { text } if text == "hello"));
    }

    #[test]
    fn text_delta_openai_spec_event_name() {
        // OpenAI Responses API spec uses "response.output_text.delta" with a bare string delta.
        let line = r#"data: {"type":"response.output_text.delta","delta":"world"}"#;
        let event = parse_line(line).unwrap();
        assert!(matches!(event, XaiEvent::TextDelta { text } if text == "world"));
    }

    #[test]
    fn text_delta_openai_spec_object_delta() {
        // OpenAI Responses API spec may also wrap delta in an object.
        let line = r#"data: {"type":"response.output_text.delta","delta":{"text":"hello"}}"#;
        let event = parse_line(line).unwrap();
        assert!(matches!(event, XaiEvent::TextDelta { text } if text == "hello"));
    }

    #[test]
    fn empty_text_delta_returns_none() {
        let line = r#"data: {"type":"message.delta","delta":{"type":"output_text","text":""}}"#;
        assert!(parse_line(line).is_none());
    }

    #[test]
    fn response_id_emitted_once() {
        let line = r#"data: {"id":"resp_123","type":"message.delta","delta":{"type":"output_text","text":"hi"}}"#;
        let events = parse_line_all(line);
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], XaiEvent::ResponseId { id } if id == "resp_123"));
        assert!(matches!(&events[1], XaiEvent::TextDelta { text } if text == "hi"));

        // Second call with same id must NOT emit ResponseId again.
        let mut emitted = true; // already emitted
        let mut pending = VecDeque::new();
        process_sse_line(line, &mut emitted, &mut pending);
        let events2: Vec<_> = pending.into_iter().collect();
        assert!(
            !events2.iter().any(|e| matches!(e, XaiEvent::ResponseId { .. })),
            "ResponseId must not be emitted twice: {events2:?}"
        );
    }

    #[test]
    fn function_call_event() {
        let line = r#"data: {"type":"function_call","function_call":{"call_id":"call_1","name":"web_search","arguments":"{\"q\":\"test\"}"}}"#;
        let event = parse_line(line).unwrap();
        match event {
            XaiEvent::FunctionCall { call_id, name, arguments } => {
                assert_eq!(call_id, "call_1");
                assert_eq!(name, "web_search");
                assert_eq!(arguments, r#"{"q":"test"}"#);
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    #[test]
    fn usage_event_from_response_completed() {
        let line = r#"data: {"type":"response.completed","usage":{"prompt_tokens":42,"completion_tokens":7}}"#;
        let event = parse_line(line).unwrap();
        assert!(
            matches!(event, XaiEvent::Usage { prompt_tokens: 42, completion_tokens: 7 }),
            "unexpected event: {event:?}"
        );
    }

    #[test]
    fn finished_event_from_response_status() {
        let line = r#"data: {"type":"response.completed","response":{"status":"completed"},"usage":{"prompt_tokens":1,"completion_tokens":1}}"#;
        let events = parse_line_all(line);
        assert!(
            events.iter().any(|e| matches!(e, XaiEvent::Finished { reason } if *reason == FinishReason::Completed)),
            "expected Finished(Completed) in {events:?}"
        );
    }

    #[test]
    fn finished_event_incomplete_status() {
        let line = r#"data: {"type":"response.completed","response":{"status":"incomplete"}}"#;
        let events = parse_line_all(line);
        assert!(
            events.iter().any(|e| matches!(e, XaiEvent::Finished { reason } if *reason == FinishReason::Incomplete)),
            "expected Finished(Incomplete) in {events:?}"
        );
    }

    #[test]
    fn unknown_event_type_returns_none() {
        let line = r#"data: {"type":"some.unknown.event","foo":"bar"}"#;
        assert!(parse_line(line).is_none());
    }
}
