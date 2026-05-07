//! HTTP client for xAI's Responses API.
//!
//! ## Endpoint choice — decision record
//!
//! xAI exposes two REST surfaces and one gRPC surface:
//! - **Responses API** at `api.x.ai/v1/responses` ← used here
//! - **OpenAI-compatible** at `api.x.ai/v1/chat/completions`
//! - **Native gRPC** defined in [`xai-org/xai-proto`](https://github.com/xai-org/xai-proto)
//!
//! The gRPC proto (`xai-org/xai-proto`) is more capable than its name suggests.
//! `chat.proto` defines a full `Chat` service: `GetCompletionChunk` for streaming,
//! `GetCompletionsRequest` with `previous_response_id` (field 22), `max_turns`
//! (field 25), `search_parameters`, `agent_count`, `use_encrypted_content`, and
//! `store_messages`. It is NOT limited to `SampleText`/`SampleTextStreaming`.
//!
//! **Key difference from the Responses API**: gRPC tools use the client-side
//! round-trip model — the streaming `Delta.tool_calls` field asks the *client* to
//! execute function calls and return results. Server-side search is requested via
//! `SearchParameters` in the request body; the model streams the answer with
//! citations but does not emit discrete tool lifecycle events. The Responses API
//! executes tools server-side and notifies the client via SSE events
//! (`response.web_search_call.completed`, etc.) without requiring a round-trip.
//!
//! This crate uses the **Responses API** for pragmatic reasons:
//!
//! 1. **SSE streaming already implemented**: the Responses API SSE parser is in
//!    production with 50+ unit tests and 104 integration tests. Switching to gRPC
//!    would require replacing it entirely with `tonic` + `prost` codegen and a new
//!    streaming layer — significant effort for no functional gain at this time.
//!
//! 2. **No official Rust gRPC SDK**: there is no xAI-maintained Rust client for
//!    the gRPC surface. The REST Responses API is the primary documented interface.
//!
//! 3. **Simpler dependency surface**: the Responses API requires only `reqwest` +
//!    `serde_json`; gRPC would add `tonic`, `prost`, `protoc`, and codegen steps.
//!
//! Revisit gRPC if xAI-exclusive features not available on the Responses API become
//! necessary: `agent_count` (multi-agent models), `use_encrypted_content` (encrypted
//! thinking), `store_messages`, or more granular `SearchParameters` (region, allowed
//! domains, safe mode, date ranges).

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream::{LocalBoxStream, StreamExt as _};
use futures_util::{Stream, stream};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::http_client::XaiHttpClient;

/// A single message in the conversation history.
///
/// Only `user`, `system`, and `assistant` (text) roles are stored — all tool
/// execution is server-side and does not produce client-visible history entries.
///
/// `content` is `Option` so that:
/// 1. Messages serialize without a `null` placeholder
///    (`skip_serializing_if = "Option::is_none"`).
/// 2. Legacy sessions stored as `{"role":"user","content":"hello"}` deserialize
///    correctly — `#[serde(default)]` fills missing fields with `None`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Message {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(text.into()),
            prompt_tokens: None,
            completion_tokens: None,
        }
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: Some(text.into()),
            prompt_tokens: None,
            completion_tokens: None,
        }
    }

    pub fn assistant_with_usage(text: impl Into<String>, prompt_tokens: u64, completion_tokens: u64) -> Self {
        Self {
            role: "assistant".to_string(),
            content: Some(text.into()),
            prompt_tokens: Some(prompt_tokens),
            completion_tokens: Some(completion_tokens),
        }
    }

    /// Returns the text content of this message, or `""` if none.
    pub fn content_str(&self) -> &str {
        self.content.as_deref().unwrap_or("")
    }
}

/// A tool specification sent in the `tools` array of a Responses API request.
///
/// - `ServerSide`: a built-in xAI tool (e.g. `"web_search"`). Serialized as
///   `{ "type": "<name>" }`.
/// - `Function`: a client-side function the runner will execute. Serialized
///   as `{ "type": "function", "function": { "name": ..., "description": ...,
///   "parameters": ... } }`.
#[derive(Clone, Debug)]
pub enum ToolSpec {
    ServerSide(String),
    Function {
        name: String,
        description: String,
        parameters: serde_json::Value,
    },
}

impl ToolSpec {
    /// Returns the tool name regardless of variant — useful in assertions.
    pub fn name(&self) -> &str {
        match self {
            Self::ServerSide(n) => n,
            Self::Function { name, .. } => name,
        }
    }
}

impl Serialize for ToolSpec {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            Self::ServerSide(name) => {
                let mut map = s.serialize_map(Some(1))?;
                map.serialize_entry("type", name)?;
                map.end()
            }
            Self::Function { name, description, parameters } => {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": description,
                        "parameters": parameters,
                    }
                })
                .serialize(s)
            }
        }
    }
}

/// An item in the `input` array sent to the Responses API.
///
/// Covers both text messages (`role` + `content`) and client-side function
/// call outputs (`type: "function_call_output"`).
#[derive(Clone, Debug)]
pub enum InputItem {
    Message { role: String, content: String },
    FunctionCallOutput { call_id: String, output: String },
}

impl Serialize for InputItem {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            Self::Message { role, content } => {
                let mut map = s.serialize_map(Some(2))?;
                map.serialize_entry("role", role)?;
                map.serialize_entry("content", content)?;
                map.end()
            }
            Self::FunctionCallOutput { call_id, output } => {
                let mut map = s.serialize_map(Some(3))?;
                map.serialize_entry("type", "function_call_output")?;
                map.serialize_entry("call_id", call_id)?;
                map.serialize_entry("output", output)?;
                map.end()
            }
        }
    }
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

    pub fn function_call_output(call_id: impl Into<String>, output: impl Into<String>) -> Self {
        Self::FunctionCallOutput { call_id: call_id.into(), output: output.into() }
    }

    /// Returns the `role` field for `Message` items, `None` for `FunctionCallOutput`.
    pub fn role(&self) -> Option<&str> {
        match self {
            Self::Message { role, .. } => Some(role),
            Self::FunctionCallOutput { .. } => None,
        }
    }

    /// Returns the `content` field for `Message` items, `None` for `FunctionCallOutput`.
    pub fn content(&self) -> Option<&str> {
        match self {
            Self::Message { content, .. } => Some(content),
            Self::FunctionCallOutput { .. } => None,
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
    /// The model stopped to wait for client-side function call results.
    ToolCalls,
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
            "tool_calls" | "requires_action" => Self::ToolCalls,
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
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    /// The `id` of this response — used as `previous_response_id` next turn.
    ResponseId { id: String },
    /// Token usage from the `response.completed` event.
    Usage {
        prompt_tokens: u64,
        completion_tokens: u64,
    },
    /// Why the model stopped — included in the `response.completed` event.
    ///
    /// `incomplete_reason` is set when `reason == Incomplete` and carries the
    /// value of `incomplete_details.reason` from the API (e.g. `"max_output_tokens"`
    /// or `"max_turns"`). Used by the agent to choose the right continuation strategy.
    Finished {
        reason: FinishReason,
        incomplete_reason: Option<String>,
    },
    /// A server-side tool call (web_search, x_search) finished on xAI's
    /// infrastructure. Emitted from `response.*_call.completed`.
    ///
    /// Carries the tool `name` (`"web_search"` or `"x_search"`) so the agent
    /// can match against `pending_tool_calls` by name (FIFO). The `item_id`
    /// in the API event differs from the `call_id` in the `function_call`
    /// event, so ID-based matching is not possible without additional state.
    ServerToolCompleted { name: String },
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
    /// Timeout for the initial HTTP round-trip (TCP connect + TLS + server sending
    /// the first response byte). Does NOT apply to the ongoing SSE stream — that
    /// is governed by `XAI_PROMPT_TIMEOUT_SECS` per-chunk in the agent layer.
    request_timeout: Duration,
}

impl Default for XaiClient {
    fn default() -> Self {
        Self::new()
    }
}

impl XaiClient {
    pub fn new() -> Self {
        let base_url =
            std::env::var("XAI_BASE_URL").unwrap_or_else(|_| "https://api.x.ai/v1".to_string());
        let request_timeout = std::env::var("XAI_PROMPT_TIMEOUT_SECS")
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

    /// Construct with an explicit base URL. Useful for tests and custom proxies.
    #[allow(dead_code)]
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
        input: &[InputItem],
        api_key: &str,
        tools: &[ToolSpec],
        previous_response_id: Option<&str>,
        max_turns: Option<u32>,
    ) -> LocalBoxStream<'static, XaiEvent> {
        debug!(
            model,
            input_len = input.len(),
            tool_count = tools.len(),
            has_prev_response = previous_response_id.is_some(),
            "xai: starting responses stream"
        );

        let result = self
            .start_request(
                model,
                input,
                api_key,
                tools,
                previous_response_id,
                max_turns,
            )
            .await;
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
        tools: &[ToolSpec],
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
                .map(|t| serde_json::to_value(t).unwrap_or_default())
                .collect();
            body["tools"] = serde_json::Value::Array(tools_json);
        }

        if let Some(prev_id) = previous_response_id {
            body["previous_response_id"] = serde_json::Value::String(prev_id.to_string());
        }

        // max_turns is only meaningful when tools are present — the server uses
        // it to cap tool-calling iterations. Skip it for tool-less requests to
        // avoid sending a parameter that has no effect.
        if !tools.is_empty()
            && let Some(turns) = max_turns
        {
            body["max_turns"] = serde_json::Value::Number(turns.into());
        }

        let response = tokio::time::timeout(
            self.request_timeout,
            self.http
                .post(format!("{}/responses", self.base_url.trim_end_matches('/')))
                .bearer_auth(api_key)
                .json(&body)
                .send(),
        )
        .await
        .map_err(|_| {
            format!(
                "xAI request timed out after {}s (no response headers received)",
                self.request_timeout.as_secs()
            )
        })?
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

#[async_trait(?Send)]
impl XaiHttpClient for XaiClient {
    async fn chat_stream(
        &self,
        model: &str,
        input: &[InputItem],
        api_key: &str,
        tools: &[ToolSpec],
        previous_response_id: Option<&str>,
        max_turns: Option<u32>,
    ) -> LocalBoxStream<'static, XaiEvent> {
        self.do_chat_stream(
            model,
            input,
            api_key,
            tools,
            previous_response_id,
            max_turns,
        )
        .await
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
    /// In-flight function call argument buffers for the OpenAI Responses API
    /// streaming events (`response.output_item.added` →
    /// `response.function_call_arguments.delta` →
    /// `response.function_call_arguments.done`).
    /// Keyed by `call_id`; value is `(name, accumulated_arguments)`.
    pending_fc: HashMap<String, (String, String)>,
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
            pending_fc: HashMap::new(),
        },
        |mut state| async move {
            loop {
                if let Some(ev) = state.pending.pop_front() {
                    return Some((ev, state));
                }

                if let Some(nl) = state.buf.find('\n') {
                    let line = state.buf[..nl].trim_end_matches('\r').to_string();
                    state.buf = state.buf[nl + 1..].to_string();
                    process_sse_line(
                        &line,
                        &mut state.response_id_emitted,
                        &mut state.pending,
                        &mut state.pending_fc,
                    );
                    continue;
                }

                match state.stream.next().await {
                    Some(Ok(chunk)) => {
                        state.buf.push_str(&String::from_utf8_lossy(&chunk));
                    }
                    Some(Err(e)) => {
                        state.pending.push_back(XaiEvent::Error {
                            message: e.to_string(),
                        });
                        return state.pending.pop_front().map(|ev| (ev, state));
                    }
                    None => {
                        let remaining = std::mem::take(&mut state.buf);
                        let line = remaining.trim();
                        if !line.is_empty() {
                            process_sse_line(
                                line,
                                &mut state.response_id_emitted,
                                &mut state.pending,
                                &mut state.pending_fc,
                            );
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
///   A `delta.reasoning_content` field (emitted by reasoning models such as
///   grok-3-mini) is logged at debug level and discarded — the ACP protocol
///   has no dedicated reasoning block type.
/// - `response.reasoning_summary_text.delta` → logged at debug level, discarded
/// - `function_call` → `FunctionCall` (complete, not streamed in fragments)
/// - `response.web_search_call.completed` / `response.x_search_call.completed`
///   → `ServerToolCompleted` (tool finished; agent advances tool to Completed)
/// - `*.in_progress` / `*.searching` variants → explicit no-op (informational; no state change)
/// - `response.completed` / `response.done` → `Usage` + `Finished`
///   These are the normal-completion events. The `status` field is parsed for
///   robustness (xAI may deviate from spec), but the primary path for
///   non-completed terminal states uses the distinct event types below.
/// - `response.incomplete` → `ResponseId` (fallback) + `Usage` + `Finished { Incomplete }`
///   Distinct event type per spec (parallel to `response.failed`). Carries
///   `incomplete_details.reason` for the continuation strategy.
/// - `response.failed` → `Finished { reason: Failed }`
/// - `response.cancelled` → `Finished { reason: Cancelled }`
///   Server-initiated cancellation (safety filter, content policy, quota).
/// - `response.error` → `Error` (mid-stream error: policy violation, safety filter)
/// - `[DONE]` → `Done`
///
/// The top-level `id` field present on most events is used to emit `ResponseId`
/// exactly once per stream (the first time it appears).
fn process_sse_line(
    line: &str,
    response_id_emitted: &mut bool,
    pending: &mut VecDeque<XaiEvent>,
    pending_fc: &mut HashMap<String, (String, String)>,
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
    if !*response_id_emitted && let Some(id) = val["id"].as_str() {
        pending.push_back(XaiEvent::ResponseId { id: id.to_string() });
        *response_id_emitted = true;
    }

    let event_type = val["type"].as_str().unwrap_or("");

    match event_type {
        // Accept both documented xAI name ("message.delta") and OpenAI spec name
        // ("response.output_text.delta"). The delta payload may be an object with
        // a "text" field or a bare string — try both.
        // Reasoning models (e.g. grok-3-mini) may also include a
        // "reasoning_content" field in the same delta object; it is logged and
        // discarded since the ACP protocol has no dedicated reasoning block type.
        "message.delta" | "response.output_text.delta" => {
            if let Some(text) = val["delta"]["text"]
                .as_str()
                .or_else(|| val["delta"].as_str())
                && !text.is_empty()
            {
                pending.push_back(XaiEvent::TextDelta {
                    text: text.to_string(),
                });
            }
            if let Some(rc) = val["delta"]["reasoning_content"].as_str()
                && !rc.is_empty()
            {
                debug!(
                    reasoning_len = rc.len(),
                    "xai: reasoning_content chunk (not forwarded to client)"
                );
            }
        }
        // OpenAI Responses API reasoning summary events — log and discard.
        "response.reasoning_summary_text.delta" => {
            let rc = val["delta"].as_str().unwrap_or("");
            if !rc.is_empty() {
                debug!(
                    reasoning_len = rc.len(),
                    "xai: reasoning summary chunk (not forwarded to client)"
                );
            }
        }
        "function_call" => {
            // xAI custom event — complete function call delivered as a single chunk.
            // {"type":"function_call","function_call":{"call_id":"...","name":"...","arguments":"..."}}
            let fc = &val["function_call"];
            let call_id = fc["call_id"].as_str().unwrap_or("").to_string();
            let name = fc["name"].as_str().unwrap_or("").to_string();
            let arguments = fc["arguments"].as_str().unwrap_or("").to_string();
            if !call_id.is_empty() || !name.is_empty() {
                pending.push_back(XaiEvent::FunctionCall {
                    call_id,
                    name,
                    arguments,
                });
            }
        }
        // ── OpenAI Responses API function-call streaming events ───────────────
        // xAI may follow the OpenAI spec and stream function calls as three
        // separate events rather than the custom single-chunk "function_call".
        // Both paths emit the same FunctionCall event; the two are not mutually
        // exclusive — a server may send the xAI custom event AND the spec events,
        // so care is taken to avoid duplicate FunctionCall emissions (the
        // done event removes the pending_fc entry and the xAI event never adds
        // to pending_fc, so no double-emit is possible).
        "response.output_item.added" => {
            // Announces a new output item. When item.type == "function_call",
            // record call_id and name so deltas can be accumulated by call_id.
            if val["item"]["type"].as_str() == Some("function_call") {
                let call_id = val["item"]["call_id"].as_str().unwrap_or("").to_string();
                let name = val["item"]["name"].as_str().unwrap_or("").to_string();
                if !call_id.is_empty() {
                    pending_fc.insert(call_id, (name, String::new()));
                }
            }
        }
        "response.function_call_arguments.delta" => {
            // Partial arguments fragment — append to the in-flight buffer.
            let call_id = val["call_id"].as_str().unwrap_or("");
            let delta = val["delta"].as_str().unwrap_or("");
            if !call_id.is_empty()
                && !delta.is_empty()
                && let Some(entry) = pending_fc.get_mut(call_id)
            {
                entry.1.push_str(delta);
            }
        }
        "response.function_call_arguments.done" => {
            // Complete function call — arguments are authoritative here.
            // Name falls back to what was recorded in output_item.added.
            let call_id = val["call_id"].as_str().unwrap_or("").to_string();
            let arguments = val["arguments"].as_str().unwrap_or("").to_string();
            let name = val["name"]
                .as_str()
                .map(str::to_string)
                .or_else(|| pending_fc.get(&call_id).map(|(n, _)| n.clone()))
                .unwrap_or_default();
            pending_fc.remove(&call_id);
            if !call_id.is_empty() || !name.is_empty() {
                pending.push_back(XaiEvent::FunctionCall {
                    call_id,
                    name,
                    arguments,
                });
            }
        }
        // ── Server-side tool call lifecycle events ───────────────────────────
        // xAI emits these while its infrastructure executes a built-in tool
        // (web_search, x_search). The `.completed` event fires as soon as the
        // search finishes — before the model begins streaming its text answer.
        // Emit ServerToolCompleted so the agent can advance the tool call to
        // Completed mid-stream rather than waiting for [DONE] (which may
        // arrive 10–20 s later after text gen).
        //
        // The in_progress and searching events are purely informational; they
        // carry no actionable data for the agent so they are dropped explicitly
        // (preferred over falling through to `_ => {}` for clarity).
        "response.web_search_call.completed" => {
            pending.push_back(XaiEvent::ServerToolCompleted {
                name: "web_search".to_string(),
            });
        }
        "response.x_search_call.completed" => {
            pending.push_back(XaiEvent::ServerToolCompleted {
                name: "x_search".to_string(),
            });
        }
        "response.web_search_call.in_progress"
        | "response.web_search_call.searching"
        | "response.x_search_call.in_progress"
        | "response.x_search_call.searching" => {}
        "response.failed" => {
            // OpenAI Responses API defines response.failed as a distinct event
            // type (not nested under response.completed). Map it to Failed so the
            // agent's stream_error path fires — otherwise the stream ends silently
            // with no Finished event and the client sees an empty success.
            pending.push_back(XaiEvent::Finished {
                reason: FinishReason::Failed,
                incomplete_reason: None,
            });
        }
        "response.incomplete" => {
            // Distinct event type per OpenAI Responses API spec (parallel to
            // response.failed). Emitted when the response was truncated before
            // completion — e.g. max_output_tokens or max_turns reached.
            //
            // Without this handler the event falls through to `_ => {}`:
            // `needs_continuation` never fires and the partial text is returned
            // to the ACP client as a successful complete turn.
            //
            // Extract the response ID (same fallback as response.completed) so
            // that the continuation request can reference this partial response
            // via `previous_response_id` without re-sending the full history.
            if !*response_id_emitted && let Some(id) = val["response"]["id"].as_str() {
                pending.push_back(XaiEvent::ResponseId { id: id.to_string() });
                *response_id_emitted = true;
            }
            let usage = if val["usage"].is_null() {
                &val["response"]["usage"]
            } else {
                &val["usage"]
            };
            let p = usage["input_tokens"]
                .as_u64()
                .or_else(|| usage["prompt_tokens"].as_u64())
                .unwrap_or(0);
            let c = usage["output_tokens"]
                .as_u64()
                .or_else(|| usage["completion_tokens"].as_u64())
                .unwrap_or(0);
            if p > 0 || c > 0 {
                pending.push_back(XaiEvent::Usage {
                    prompt_tokens: p,
                    completion_tokens: c,
                });
            }
            let incomplete_reason = val["response"]["incomplete_details"]["reason"]
                .as_str()
                .or_else(|| val["incomplete_details"]["reason"].as_str())
                .map(str::to_string);
            pending.push_back(XaiEvent::Finished {
                reason: FinishReason::Incomplete,
                incomplete_reason,
            });
        }
        "response.cancelled" => {
            // Distinct event type per OpenAI Responses API spec. Emitted when the
            // server cancels a response (safety filter, content policy, quota).
            // Note: this is server-initiated cancellation, not the client-side
            // cancel triggered by XaiAgent::cancel(). Without this handler the
            // event falls through to `_ => {}` and the prompt completes silently
            // with whatever partial text was accumulated — the ACP client would
            // see a success instead of an error.
            pending.push_back(XaiEvent::Finished {
                reason: FinishReason::Cancelled,
                incomplete_reason: None,
            });
        }
        "response.error" => {
            // Mid-stream error event (e.g. content policy violation, safety filter).
            // Distinct from response.failed (which signals a complete-response failure).
            // Map to XaiEvent::Error so the agent's stream_error path fires and the
            // orphaned user message is compensated — without this the stream ends
            // silently and the client sees an empty successful response.
            let message = val["error"]["message"]
                .as_str()
                .or_else(|| val["message"].as_str())
                .unwrap_or("xAI stream error")
                .to_string();
            pending.push_back(XaiEvent::Error { message });
        }
        "response.completed" | "response.done" => {
            // The OpenAI Responses API spec puts the response ID at response.id
            // (nested), not at the top-level id field. xAI's streaming events
            // (message.delta, function_call) do carry a top-level id, so the
            // `response_id_emitted` guard at the top of this function handles the
            // common path. But a stream that emits only response.completed — e.g. a
            // pure server-side tool turn where no text is streamed, or an empty
            // continuation — would never see a top-level id and would leave
            // current_response_id as None, forcing full-history fallback on the
            // next turn. Extract the nested id here as a fallback.
            if !*response_id_emitted && let Some(id) = val["response"]["id"].as_str() {
                pending.push_back(XaiEvent::ResponseId { id: id.to_string() });
                *response_id_emitted = true;
            }
            // Usage may be top-level (xAI extension) or nested inside the
            // response object (per OpenAI Responses API spec).
            // Field names: Responses API uses input_tokens/output_tokens;
            // Chat Completions API uses prompt_tokens/completion_tokens.
            // Accept both for forward compatibility.
            let usage = if val["usage"].is_null() {
                &val["response"]["usage"]
            } else {
                &val["usage"]
            };
            let p = usage["input_tokens"]
                .as_u64()
                .or_else(|| usage["prompt_tokens"].as_u64())
                .unwrap_or(0);
            let c = usage["output_tokens"]
                .as_u64()
                .or_else(|| usage["completion_tokens"].as_u64())
                .unwrap_or(0);
            if p > 0 || c > 0 {
                pending.push_back(XaiEvent::Usage {
                    prompt_tokens: p,
                    completion_tokens: c,
                });
            }
            // Emit the finish reason from response.status (Responses API field).
            // Falls back to checking top-level status for xAI-specific deviations.
            // When status is "incomplete", also extract incomplete_details.reason
            // (e.g. "max_output_tokens" or "max_turns") so the agent can choose
            // the right continuation strategy.
            let status = val["response"]["status"]
                .as_str()
                .or_else(|| val["status"].as_str());
            if let Some(s) = status {
                let incomplete_reason = if s == "incomplete" {
                    val["response"]["incomplete_details"]["reason"]
                        .as_str()
                        .or_else(|| val["incomplete_details"]["reason"].as_str())
                        .map(str::to_string)
                } else {
                    None
                };
                pending.push_back(XaiEvent::Finished {
                    reason: FinishReason::from_status(s),
                    incomplete_reason,
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
        let mut pending_fc = HashMap::new();
        process_sse_line(line, &mut emitted, &mut pending, &mut pending_fc);
        pending.pop_front()
    }

    /// Parse a line and return ALL emitted events.
    fn parse_line_all(line: &str) -> Vec<XaiEvent> {
        let mut emitted = false;
        let mut pending = VecDeque::new();
        let mut pending_fc = HashMap::new();
        process_sse_line(line, &mut emitted, &mut pending, &mut pending_fc);
        pending.into_iter().collect()
    }

    /// Parse a sequence of lines sharing state (for stateful streaming events).
    fn parse_lines(lines: &[&str]) -> Vec<XaiEvent> {
        let mut emitted = false;
        let mut pending = VecDeque::new();
        let mut pending_fc = HashMap::new();
        for &line in lines {
            process_sse_line(line, &mut emitted, &mut pending, &mut pending_fc);
        }
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
        let line =
            r#"data: {"type":"message.delta","delta":{"type":"output_text","text":"hello"}}"#;
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
        let mut pending_fc = HashMap::new();
        process_sse_line(line, &mut emitted, &mut pending, &mut pending_fc);
        let events2: Vec<_> = pending.into_iter().collect();
        assert!(
            !events2
                .iter()
                .any(|e| matches!(e, XaiEvent::ResponseId { .. })),
            "ResponseId must not be emitted twice: {events2:?}"
        );
    }

    #[test]
    fn function_call_event() {
        let line = r#"data: {"type":"function_call","function_call":{"call_id":"call_1","name":"web_search","arguments":"{\"q\":\"test\"}"}}"#;
        let event = parse_line(line).unwrap();
        match event {
            XaiEvent::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                assert_eq!(call_id, "call_1");
                assert_eq!(name, "web_search");
                assert_eq!(arguments, r#"{"q":"test"}"#);
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    #[test]
    fn usage_event_from_response_completed_legacy_fields() {
        // Chat Completions API field names (prompt_tokens / completion_tokens)
        let line = r#"data: {"type":"response.completed","usage":{"prompt_tokens":42,"completion_tokens":7}}"#;
        let event = parse_line(line).unwrap();
        assert!(
            matches!(
                event,
                XaiEvent::Usage {
                    prompt_tokens: 42,
                    completion_tokens: 7
                }
            ),
            "unexpected event: {event:?}"
        );
    }

    #[test]
    fn usage_event_from_response_completed_responses_api_fields() {
        // Responses API field names (input_tokens / output_tokens) used by grok-4+
        let line = r#"data: {"type":"response.completed","usage":{"input_tokens":100,"output_tokens":50}}"#;
        let event = parse_line(line).unwrap();
        assert!(
            matches!(
                event,
                XaiEvent::Usage {
                    prompt_tokens: 100,
                    completion_tokens: 50
                }
            ),
            "unexpected event: {event:?}"
        );
    }

    #[test]
    fn finished_event_from_response_status() {
        let line = r#"data: {"type":"response.completed","response":{"status":"completed"},"usage":{"prompt_tokens":1,"completion_tokens":1}}"#;
        let events = parse_line_all(line);
        assert!(
            events.iter().any(|e| matches!(e, XaiEvent::Finished { reason, .. } if *reason == FinishReason::Completed)),
            "expected Finished(Completed) in {events:?}"
        );
    }

    #[test]
    fn finished_event_incomplete_status() {
        let line = r#"data: {"type":"response.completed","response":{"status":"incomplete"}}"#;
        let events = parse_line_all(line);
        assert!(
            events.iter().any(|e| matches!(e, XaiEvent::Finished { reason, .. } if *reason == FinishReason::Incomplete)),
            "expected Finished(Incomplete) in {events:?}"
        );
    }

    #[test]
    fn finished_incomplete_reason_max_output_tokens() {
        let line = r#"data: {"type":"response.completed","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}"#;
        let events = parse_line_all(line);
        assert!(
            events.iter().any(|e| matches!(
                e,
                XaiEvent::Finished { reason, incomplete_reason }
                    if *reason == FinishReason::Incomplete
                    && incomplete_reason.as_deref() == Some("max_output_tokens")
            )),
            "expected Finished(Incomplete, max_output_tokens) in {events:?}"
        );
    }

    #[test]
    fn finished_incomplete_reason_max_turns() {
        let line = r#"data: {"type":"response.completed","response":{"status":"incomplete","incomplete_details":{"reason":"max_turns"}}}"#;
        let events = parse_line_all(line);
        assert!(
            events.iter().any(|e| matches!(
                e,
                XaiEvent::Finished { reason, incomplete_reason }
                    if *reason == FinishReason::Incomplete
                    && incomplete_reason.as_deref() == Some("max_turns")
            )),
            "expected Finished(Incomplete, max_turns) in {events:?}"
        );
    }

    #[test]
    fn finished_incomplete_top_level_reason() {
        // xAI may put incomplete_details at the top level instead of nested in response.
        let line = r#"data: {"type":"response.completed","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}"#;
        let events = parse_line_all(line);
        assert!(
            events.iter().any(|e| matches!(
                e,
                XaiEvent::Finished { reason, incomplete_reason }
                    if *reason == FinishReason::Incomplete
                    && incomplete_reason.as_deref() == Some("max_output_tokens")
            )),
            "expected Finished(Incomplete, max_output_tokens) from top-level in {events:?}"
        );
    }

    #[test]
    fn unknown_event_type_returns_none() {
        let line = r#"data: {"type":"some.unknown.event","foo":"bar"}"#;
        assert!(parse_line(line).is_none());
    }

    // ── OpenAI Responses API function-call streaming events ───────────────────

    #[test]
    fn function_call_arguments_done_emits_function_call() {
        // response.function_call_arguments.done carries the complete call.
        let line = r#"data: {"type":"response.function_call_arguments.done","call_id":"call_abc","name":"web_search","arguments":"{\"q\":\"rust\"}"}"#;
        let event = parse_line(line).unwrap();
        match event {
            XaiEvent::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                assert_eq!(call_id, "call_abc");
                assert_eq!(name, "web_search");
                assert_eq!(arguments, r#"{"q":"rust"}"#);
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    #[test]
    fn function_call_streaming_sequence_output_item_added_delta_done() {
        // Full three-event OpenAI spec sequence: added → delta → done.
        let added = r#"data: {"type":"response.output_item.added","item":{"type":"function_call","call_id":"call_1","name":"web_search"}}"#;
        let delta1 = r#"data: {"type":"response.function_call_arguments.delta","call_id":"call_1","delta":"{\"q\":"}"#;
        let delta2 = r#"data: {"type":"response.function_call_arguments.delta","call_id":"call_1","delta":"\"test\"}"}"#;
        let done = r#"data: {"type":"response.function_call_arguments.done","call_id":"call_1","name":"web_search","arguments":"{\"q\":\"test\"}"}"#;

        let events = parse_lines(&[added, delta1, delta2, done]);
        // Only the done event emits a FunctionCall (added/delta are stateful, no events).
        assert_eq!(
            events.len(),
            1,
            "expected exactly one FunctionCall: {events:?}"
        );
        match &events[0] {
            XaiEvent::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                assert_eq!(call_id, "call_1");
                assert_eq!(name, "web_search");
                assert_eq!(arguments, r#"{"q":"test"}"#);
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    #[test]
    fn function_call_done_without_prior_added_still_emits() {
        // done without a preceding output_item.added — name comes from done itself.
        let done = r#"data: {"type":"response.function_call_arguments.done","call_id":"call_2","name":"x_search","arguments":"{}"}"#;
        let event = parse_line(done).unwrap();
        assert!(matches!(event, XaiEvent::FunctionCall { name, .. } if name == "x_search"));
    }

    #[test]
    fn output_item_added_non_function_call_emits_nothing() {
        // output_item.added for text items must not emit any event.
        let line =
            r#"data: {"type":"response.output_item.added","item":{"type":"message","id":"msg_1"}}"#;
        assert!(parse_line(line).is_none());
    }

    #[test]
    fn response_failed_event_emits_finished_failed() {
        let line = r#"data: {"type":"response.failed","response":{"id":"resp_1","status":"failed","error":{"code":"server_error","message":"Internal error"}}}"#;
        let event = parse_line(line).unwrap();
        assert!(
            matches!(event, XaiEvent::Finished { ref reason, .. } if *reason == FinishReason::Failed),
            "expected Finished(Failed), got {event:?}"
        );
    }

    #[test]
    fn response_incomplete_event_emits_finished_incomplete() {
        let line = r#"data: {"type":"response.incomplete","response":{"id":"resp_trunc","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"usage":{"prompt_tokens":10,"completion_tokens":50}}}"#;
        let events = parse_line_all(line);
        let id_events: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                XaiEvent::ResponseId { id } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            id_events,
            vec!["resp_trunc"],
            "response.incomplete must emit ResponseId"
        );
        let usage_event = events.iter().find(|e| matches!(e, XaiEvent::Usage { .. }));
        assert!(usage_event.is_some(), "response.incomplete must emit Usage");
        let finished = events.iter().find_map(|e| match e {
            XaiEvent::Finished {
                reason,
                incomplete_reason,
            } => Some((reason.clone(), incomplete_reason.clone())),
            _ => None,
        });
        let (reason, inc_reason) = finished.expect("response.incomplete must emit Finished");
        assert_eq!(reason, FinishReason::Incomplete);
        assert_eq!(inc_reason.as_deref(), Some("max_output_tokens"));
    }

    #[test]
    fn response_incomplete_event_no_incomplete_details() {
        let line = r#"data: {"type":"response.incomplete","response":{"id":"resp_2","status":"incomplete"}}"#;
        let events = parse_line_all(line);
        let finished = events.iter().find_map(|e| match e {
            XaiEvent::Finished {
                reason,
                incomplete_reason,
            } => Some((reason.clone(), incomplete_reason.clone())),
            _ => None,
        });
        let (reason, inc_reason) = finished.expect("response.incomplete must emit Finished");
        assert_eq!(reason, FinishReason::Incomplete);
        assert!(inc_reason.is_none(), "no incomplete_details → None");
    }

    #[test]
    fn response_cancelled_event_emits_finished_cancelled() {
        let line = r#"data: {"type":"response.cancelled","response":{"id":"resp_x","status":"cancelled"}}"#;
        let event = parse_line(line).unwrap();
        assert!(
            matches!(event, XaiEvent::Finished { ref reason, .. } if *reason == FinishReason::Cancelled),
            "expected Finished(Cancelled), got {event:?}"
        );
    }

    #[test]
    fn response_error_event_emits_error() {
        // Mid-stream error (policy violation, safety filter, etc.).
        let line = r#"data: {"type":"response.error","error":{"code":"content_policy","message":"Request blocked by content policy"}}"#;
        let event = parse_line(line).unwrap();
        match event {
            XaiEvent::Error { message } => {
                assert!(
                    message.contains("content policy"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn response_error_event_fallback_message() {
        // Fallback when error object is absent or oddly shaped.
        let line = r#"data: {"type":"response.error","message":"unexpected failure"}"#;
        let event = parse_line(line).unwrap();
        assert!(
            matches!(event, XaiEvent::Error { .. }),
            "expected Error, got {event:?}"
        );
    }

    #[test]
    fn response_completed_nested_id_emitted_when_not_yet_captured() {
        // When response.completed is the first (or only) event and carries no
        // top-level id, the nested response.id must be extracted so the agent
        // can use previous_response_id on the next turn.
        let line = r#"data: {"type":"response.completed","response":{"id":"resp_nested","status":"completed"}}"#;
        let events = parse_lines(&[line]);
        let id_events: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                XaiEvent::ResponseId { id } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            id_events,
            vec!["resp_nested"],
            "nested response.id must be emitted as ResponseId"
        );
    }

    #[test]
    fn response_completed_nested_id_not_duplicated_when_already_captured() {
        // If a top-level id was already emitted (from a message.delta), the nested
        // id in response.completed must NOT produce a second ResponseId event.
        let lines = &[
            r#"data: {"id":"resp_toplevel","type":"message.delta","delta":{"type":"output_text","text":"hi"}}"#,
            r#"data: {"type":"response.completed","response":{"id":"resp_nested","status":"completed"}}"#,
        ];
        let events = parse_lines(lines);
        let id_events: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                XaiEvent::ResponseId { id } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        // Only the top-level id from message.delta; the nested one is suppressed.
        assert_eq!(id_events, vec!["resp_toplevel"]);
    }

    // ── search call lifecycle events ──────────────────────────────────────────

    #[test]
    fn web_search_call_completed_emits_search_call_completed() {
        let line = r#"data: {"type":"response.web_search_call.completed","item_id":"ws_abc","output_index":0,"sequence_number":5}"#;
        let event = parse_line(line).unwrap();
        assert!(
            matches!(event, XaiEvent::ServerToolCompleted { ref name } if name == "web_search"),
            "expected ServerToolCompleted(web_search), got {event:?}"
        );
    }

    #[test]
    fn x_search_call_completed_emits_search_call_completed() {
        let line = r#"data: {"type":"response.x_search_call.completed","item_id":"xs_abc","output_index":0,"sequence_number":5}"#;
        let event = parse_line(line).unwrap();
        assert!(
            matches!(event, XaiEvent::ServerToolCompleted { ref name } if name == "x_search"),
            "expected ServerToolCompleted(x_search), got {event:?}"
        );
    }

    #[test]
    fn web_search_call_in_progress_is_silent() {
        let line = r#"data: {"type":"response.web_search_call.in_progress","item_id":"ws_abc","output_index":0}"#;
        assert!(
            parse_line(line).is_none(),
            "web_search_call.in_progress must produce no event"
        );
    }

    #[test]
    fn web_search_call_searching_is_silent() {
        let line = r#"data: {"type":"response.web_search_call.searching","item_id":"ws_abc","output_index":0}"#;
        assert!(
            parse_line(line).is_none(),
            "web_search_call.searching must produce no event"
        );
    }

    #[test]
    fn x_search_call_in_progress_is_silent() {
        let line = r#"data: {"type":"response.x_search_call.in_progress","item_id":"xs_abc","output_index":0}"#;
        assert!(
            parse_line(line).is_none(),
            "x_search_call.in_progress must produce no event"
        );
    }

    #[test]
    fn x_search_call_searching_is_silent() {
        let line = r#"data: {"type":"response.x_search_call.searching","item_id":"xs_abc","output_index":0}"#;
        assert!(
            parse_line(line).is_none(),
            "x_search_call.searching must produce no event"
        );
    }

    // ── reasoning content ─────────────────────────────────────────────────────

    #[test]
    fn text_delta_with_reasoning_content_emits_text_delta() {
        // grok-3-mini sends both fields in the same delta object.
        // The text must be forwarded; reasoning_content must be discarded silently.
        let line = r#"data: {"type":"message.delta","delta":{"text":"hello","reasoning_content":"Let me think..."}}"#;
        let event = parse_line(line).unwrap();
        assert!(
            matches!(event, XaiEvent::TextDelta { ref text } if text == "hello"),
            "text field must produce TextDelta even when reasoning_content is also present: {event:?}"
        );
    }

    #[test]
    fn reasoning_content_delta_emits_nothing() {
        // Reasoning models (e.g. grok-3-mini) emit delta.reasoning_content.
        // This must NOT produce a TextDelta — reasoning is not forwarded to the ACP client.
        let line =
            r#"data: {"type":"message.delta","delta":{"reasoning_content":"Let me think..."}}"#;
        assert!(
            parse_line(line).is_none(),
            "reasoning_content-only delta must not produce any event"
        );
    }

    #[test]
    fn reasoning_summary_text_delta_emits_nothing() {
        // OpenAI Responses API reasoning summary events are logged and discarded.
        let line =
            r#"data: {"type":"response.reasoning_summary_text.delta","delta":"Summary so far."}"#;
        assert!(
            parse_line(line).is_none(),
            "response.reasoning_summary_text.delta must produce no event"
        );
    }

    // ── response.done event type ──────────────────────────────────────────────

    #[test]
    fn response_done_event_same_as_completed() {
        // "response.done" shares the same match arm as "response.completed".
        let line = r#"data: {"type":"response.done","response":{"status":"completed"},"usage":{"prompt_tokens":5,"completion_tokens":3}}"#;
        let events = parse_line_all(line);
        assert!(
            events.iter().any(|e| matches!(e, XaiEvent::Finished { reason, .. } if *reason == FinishReason::Completed)),
            "response.done must emit Finished(Completed) just like response.completed: {events:?}"
        );
    }

    // ── usage nested inside response object ───────────────────────────────────

    #[test]
    fn usage_nested_in_response_object() {
        // When top-level "usage" is absent (null), the parser must fall back to
        // response.usage per the OpenAI Responses API spec.
        let line = r#"data: {"type":"response.completed","response":{"id":"r1","status":"completed","usage":{"prompt_tokens":10,"completion_tokens":20}}}"#;
        let events = parse_line_all(line);
        assert!(
            events.iter().any(|e| matches!(
                e,
                XaiEvent::Usage {
                    prompt_tokens: 10,
                    completion_tokens: 20
                }
            )),
            "usage nested in response object must be extracted: {events:?}"
        );
    }

    // ── response.completed with non-completed status ──────────────────────────

    #[test]
    fn response_completed_with_failed_status_emits_finished_failed() {
        // response.completed can carry any status — "failed" hits FinishReason::Failed
        // via the same path as response.completed/status=completed, distinct from the
        // dedicated response.failed event handler.
        let line = r#"data: {"type":"response.completed","response":{"status":"failed"}}"#;
        let events = parse_line_all(line);
        assert!(
            events.iter().any(|e| matches!(e, XaiEvent::Finished { reason, .. } if *reason == FinishReason::Failed)),
            "response.completed with status=failed must emit Finished(Failed): {events:?}"
        );
    }

    #[test]
    fn response_completed_with_cancelled_status_emits_finished_cancelled() {
        let line = r#"data: {"type":"response.completed","response":{"status":"cancelled"}}"#;
        let events = parse_line_all(line);
        assert!(
            events.iter().any(|e| matches!(e, XaiEvent::Finished { reason, .. } if *reason == FinishReason::Cancelled)),
            "response.completed with status=cancelled must emit Finished(Cancelled): {events:?}"
        );
    }

    // ── function_call empty guard ─────────────────────────────────────────────

    #[test]
    fn function_call_with_empty_call_id_and_name_emits_nothing() {
        // Both call_id and name empty — the guard `!call_id.is_empty() || !name.is_empty()`
        // is false, so no FunctionCall event must be emitted.
        let line = r#"data: {"type":"function_call","function_call":{"call_id":"","name":"","arguments":"{}"}}"#;
        assert!(
            parse_line(line).is_none(),
            "function_call with empty call_id and name must produce no event"
        );
    }

    // ── function_call_arguments.delta for unknown call_id ─────────────────────

    #[test]
    fn function_call_arguments_delta_unknown_call_id_is_ignored() {
        // A delta that arrives without a preceding output_item.added for its
        // call_id must not panic or produce any event — the entry is simply absent
        // from pending_fc so the delta is a no-op.
        let delta = r#"data: {"type":"response.function_call_arguments.delta","call_id":"ghost_id","delta":"{\"q\":"}"#;
        assert!(
            parse_line(delta).is_none(),
            "delta for unknown call_id must produce no event"
        );
    }

    #[test]
    fn function_call_arguments_delta_empty_call_id_is_ignored() {
        // `call_id` is empty string — the guard `!call_id.is_empty()` rejects it,
        // so the delta must be silently discarded without panicking.
        let delta = r#"data: {"type":"response.function_call_arguments.delta","call_id":"","delta":"{\"q\":"}"#;
        assert!(
            parse_line(delta).is_none(),
            "delta with empty call_id must produce no event"
        );
    }

    // ── response.error event → XaiEvent::Error ───────────────────────────────

    #[test]
    fn response_error_emits_xai_error_with_nested_message() {
        // Primary path: message lives under error.message (OpenAI Responses API spec).
        let line =
            r#"data: {"type":"response.error","error":{"message":"content policy violation"}}"#;
        let event = parse_line(line).expect("response.error must emit an event");
        assert!(
            matches!(&event, XaiEvent::Error { message } if message == "content policy violation"),
            "must carry the nested error message: {event:?}"
        );
    }

    #[test]
    fn response_error_falls_back_to_top_level_message() {
        // Fallback: message at top level (xAI deviation from spec).
        // `val["error"]["message"]` is absent, `.or_else(|| val["message"])` fires.
        let line = r#"data: {"type":"response.error","message":"rate limit exceeded"}"#;
        let event = parse_line(line).expect("response.error must emit an event");
        assert!(
            matches!(&event, XaiEvent::Error { message } if message == "rate limit exceeded"),
            "must fall back to top-level message field: {event:?}"
        );
    }

    #[test]
    fn response_error_uses_default_message_when_no_field_present() {
        // Both fallback paths absent — `.unwrap_or("xAI stream error")` fires.
        let line = r#"data: {"type":"response.error"}"#;
        let event =
            parse_line(line).expect("response.error must emit an event even without a message");
        assert!(
            matches!(&event, XaiEvent::Error { message } if message == "xAI stream error"),
            "must use the default message when no message field present: {event:?}"
        );
    }

    // ── FinishReason::Other for unknown status ────────────────────────────────

    #[test]
    fn response_completed_with_unknown_status_emits_finished_other() {
        // The `from_status` catch-all arm: any unrecognised status string must
        // produce `FinishReason::Other(status)` rather than panicking or defaulting
        // to Completed. The agent treats all `Finished` variants as end-of-turn, so
        // the loop will break correctly for any future xAI status extensions.
        let line = r#"data: {"type":"response.completed","response":{"status":"in_progress"}}"#;
        let events = parse_line_all(line);
        let finished = events.iter().find_map(|e| {
            if let XaiEvent::Finished { reason, .. } = e {
                Some(reason)
            } else {
                None
            }
        });
        assert!(
            matches!(finished, Some(FinishReason::Other(s)) if s == "in_progress"),
            "unknown status must map to FinishReason::Other(status), got: {finished:?}"
        );
    }

    // ── response.output_item.added: non-function-call type ignored ────────────

    #[test]
    fn output_item_added_with_text_type_produces_no_event() {
        // When the server announces a text output item (type != "function_call"),
        // the inner `if item.type == "function_call"` guard must reject it and
        // produce no event. Without the guard a stale entry would be left in
        // `pending_fc` and the next delta for an unrelated call_id could be
        // mis-attributed.
        let line =
            r#"data: {"type":"response.output_item.added","item":{"type":"text","id":"item_abc"}}"#;
        assert!(
            parse_line(line).is_none(),
            "output_item.added with type=text must produce no event"
        );
    }

    #[test]
    fn output_item_added_with_message_type_produces_no_event() {
        // Same guard, different non-function-call item type.
        let line =
            r#"data: {"type":"response.output_item.added","item":{"type":"message","id":"msg_1"}}"#;
        assert!(
            parse_line(line).is_none(),
            "output_item.added with type=message must produce no event"
        );
    }

    // ── parse_sse: streaming state machine ────────────────────────────────────

    #[tokio::test]
    async fn parse_sse_trailing_buffer_without_newline() {
        use futures_util::stream::{self, StreamExt as _};

        // Simulate a stream that ends without a trailing newline.
        // The `None` branch in parse_sse trims and processes the remaining buffer.
        let bytes = Bytes::from("data: [DONE]"); // no \n
        let s = stream::iter(vec![Ok::<_, reqwest::Error>(bytes)]);
        let events: Vec<_> = parse_sse(s).collect().await;
        assert!(
            events.iter().any(|e| matches!(e, XaiEvent::Done)),
            "parse_sse must process a trailing buffer that has no newline: {events:?}"
        );
    }

    #[tokio::test]
    async fn parse_sse_data_split_across_chunks() {
        use futures_util::stream::{self, StreamExt as _};

        // The SSE parser must accumulate partial byte chunks until it finds a
        // newline delimiter, then process the complete line.
        let chunk1 = Bytes::from("data: {\"type\":\"message.delta\",\"delta\":{\"text\":\"hel");
        let chunk2 = Bytes::from("lo\"}}\n");
        let s = stream::iter(vec![
            Ok::<_, reqwest::Error>(chunk1),
            Ok::<_, reqwest::Error>(chunk2),
        ]);
        let events: Vec<_> = parse_sse(s).collect().await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, XaiEvent::TextDelta { text } if text == "hello")),
            "parse_sse must reassemble lines split across multiple byte chunks: {events:?}"
        );
    }

    #[tokio::test]
    async fn parse_sse_crlf_line_endings_stripped() {
        use futures_util::stream::{self, StreamExt as _};

        // SSE spec allows \r\n line endings. The parser strips the trailing \r.
        let bytes = Bytes::from("data: [DONE]\r\n");
        let s = stream::iter(vec![Ok::<_, reqwest::Error>(bytes)]);
        let events: Vec<_> = parse_sse(s).collect().await;
        assert!(
            events.iter().any(|e| matches!(e, XaiEvent::Done)),
            "parse_sse must strip \\r from CRLF lines: {events:?}"
        );
    }

    // ── multiple concurrent function calls in a single stream ────────────────

    #[test]
    fn multiple_concurrent_function_calls_emitted_independently() {
        // Two function calls are interleaved in a single stream. The parser must
        // track both in `pending_fc` by call_id and emit each FunctionCall event
        // independently when its own `done` event arrives.
        //
        // Stream sequence:
        //   added(call_1, search)
        //   added(call_2, lookup)
        //   delta(call_1, first fragment)
        //   delta(call_2, first fragment)
        //   delta(call_1, second fragment)
        //   done(call_1)  → FunctionCall { call_1, search, '{"q":"rust"}' }
        //   delta(call_2, second fragment)
        //   done(call_2)  → FunctionCall { call_2, lookup, '{"id":"42"}' }
        let lines = [
            r#"data: {"type":"response.output_item.added","item":{"type":"function_call","call_id":"call_1","name":"search"}}"#,
            r#"data: {"type":"response.output_item.added","item":{"type":"function_call","call_id":"call_2","name":"lookup"}}"#,
            r#"data: {"type":"response.function_call_arguments.delta","call_id":"call_1","delta":"{\"q\":"}"#,
            r#"data: {"type":"response.function_call_arguments.delta","call_id":"call_2","delta":"{\"id\":"}"#,
            r#"data: {"type":"response.function_call_arguments.delta","call_id":"call_1","delta":"\"rust\"}"}"#,
            r#"data: {"type":"response.function_call_arguments.done","call_id":"call_1","name":"search","arguments":"{\"q\":\"rust\"}"}"#,
            r#"data: {"type":"response.function_call_arguments.delta","call_id":"call_2","delta":"\"42\"}"}"#,
            r#"data: {"type":"response.function_call_arguments.done","call_id":"call_2","name":"lookup","arguments":"{\"id\":\"42\"}"}"#,
        ];

        let mut emitted = false;
        let mut pending = VecDeque::new();
        let mut pending_fc = HashMap::new();
        for line in &lines {
            process_sse_line(line, &mut emitted, &mut pending, &mut pending_fc);
        }

        let events: Vec<_> = pending.drain(..).collect();
        assert_eq!(
            events.len(),
            2,
            "must emit exactly 2 FunctionCall events, got: {events:?}"
        );
        assert!(
            matches!(&events[0], XaiEvent::FunctionCall { call_id, name, arguments }
                if call_id == "call_1" && name == "search" && arguments == r#"{"q":"rust"}"#),
            "first event must be call_1/search with correct arguments: {:?}",
            events[0]
        );
        assert!(
            matches!(&events[1], XaiEvent::FunctionCall { call_id, name, arguments }
                if call_id == "call_2" && name == "lookup" && arguments == r#"{"id":"42"}"#),
            "second event must be call_2/lookup with correct arguments: {:?}",
            events[1]
        );
        assert!(
            pending_fc.is_empty(),
            "pending_fc must be empty after all done events"
        );
    }

    // ── response.completed: top-level incomplete_details fallback ─────────────

    #[test]
    fn response_completed_with_top_level_incomplete_status_and_reason() {
        // When "status" and "incomplete_details" appear at the top level of the
        // event (no "response" wrapper), both fallback paths must fire:
        //   val["response"]["status"] → null → falls back to val["status"]
        //   val["response"]["incomplete_details"]["reason"] → null → falls back to val["incomplete_details"]["reason"]
        //
        // Some xAI deviations from the OpenAI Responses API spec place fields at
        // the event root rather than nested under a "response" object.
        let line = r#"data: {"type":"response.completed","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}"#;
        let events = parse_line_all(line);

        let finished = events.iter().find_map(|e| {
            if let XaiEvent::Finished {
                reason,
                incomplete_reason,
            } = e
            {
                Some((reason, incomplete_reason))
            } else {
                None
            }
        });

        let (reason, incomplete_reason) =
            finished.expect("response.completed with status=incomplete must emit a Finished event");
        assert_eq!(
            *reason,
            FinishReason::Incomplete,
            "top-level status=incomplete must map to Incomplete"
        );
        assert_eq!(
            incomplete_reason.as_deref(),
            Some("max_output_tokens"),
            "incomplete_reason must be extracted from top-level incomplete_details.reason"
        );
    }

    // ── usage: asymmetric tokens still emits ─────────────────────────────────

    #[test]
    fn usage_with_only_prompt_tokens_nonzero_emits_usage() {
        // `p > 0 || c > 0` — when completion_tokens is zero but prompt_tokens > 0
        // the guard must pass and Usage must be emitted.
        let line = r#"data: {"type":"response.completed","response":{"status":"completed"},"usage":{"prompt_tokens":5,"completion_tokens":0}}"#;
        let events = parse_line_all(line);
        assert!(
            events.iter().any(|e| matches!(
                e,
                XaiEvent::Usage {
                    prompt_tokens: 5,
                    completion_tokens: 0
                }
            )),
            "asymmetric usage (p=5, c=0) must still be emitted: {events:?}"
        );
    }

    // ── function_call done: empty arguments emits with empty string ───────────

    #[test]
    fn function_call_done_with_missing_arguments_emits_empty_string() {
        // `val["arguments"].as_str().unwrap_or("")` returns "" when the field is
        // absent. The event must still be emitted — not suppressed — so callers
        // can observe a function call with no argument data.
        let done = r#"data: {"type":"response.function_call_arguments.done","call_id":"call_1","name":"my_tool"}"#;
        let event = parse_line(done).expect("done without arguments must still emit FunctionCall");
        match event {
            XaiEvent::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                assert_eq!(call_id, "call_1");
                assert_eq!(name, "my_tool");
                assert_eq!(
                    arguments, "",
                    "absent arguments field must produce empty string"
                );
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    // ── output_item.added: re-announced call_id overwrites name ──────────────

    #[test]
    fn output_item_added_reannounce_same_call_id_overwrites_name() {
        // When the server sends two `output_item.added` events for the same
        // call_id (re-announcement with a corrected name), the second insert into
        // `pending_fc` overwrites the first. The `done` event must use the last name.
        let added1 = r#"data: {"type":"response.output_item.added","item":{"type":"function_call","call_id":"call_r","name":"wrong_name"}}"#;
        let added2 = r#"data: {"type":"response.output_item.added","item":{"type":"function_call","call_id":"call_r","name":"correct_name"}}"#;
        let done = r#"data: {"type":"response.function_call_arguments.done","call_id":"call_r","arguments":"{}"}"#;
        let events = parse_lines(&[added1, added2, done]);
        assert_eq!(events.len(), 1, "only the done event must emit: {events:?}");
        match &events[0] {
            XaiEvent::FunctionCall { name, .. } => {
                assert_eq!(
                    name, "correct_name",
                    "second output_item.added must overwrite the name"
                );
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    // ── parse_sse: event split across chunk boundary ──────────────────────────

    #[tokio::test]
    async fn parse_sse_event_split_across_chunk_boundary() {
        use futures_util::stream::{self, StreamExt as _};

        // A single SSE line split into two chunks at an arbitrary byte boundary.
        // The buffer accumulation logic must reassemble before parsing.
        let part1 = Bytes::from("data: {\"type\":\"message.delta\",\"delta\":{\"type\":\"output");
        let part2 = Bytes::from("_text\",\"text\":\"hello\"}}\n");
        let s = stream::iter(vec![
            Ok::<_, reqwest::Error>(part1),
            Ok::<_, reqwest::Error>(part2),
        ]);
        let events: Vec<_> = parse_sse(s).collect().await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, XaiEvent::TextDelta { text } if text == "hello")),
            "SSE event split across chunk boundary must be reassembled: {events:?}"
        );
    }

    // ── response.completed: zero-token usage guard ───────────────────────────

    #[test]
    fn response_completed_zero_usage_emits_no_usage_event() {
        // The emission guard `if p > 0 || c > 0` must skip Usage when both
        // tokens are zero — a stream that returned no content must not emit a
        // spurious Usage event with all-zero counts.
        let line = r#"data: {"type":"response.completed","response":{"id":"r1","status":"completed"},"usage":{"prompt_tokens":0,"completion_tokens":0}}"#;
        let events = parse_line_all(line);
        assert!(
            !events.iter().any(|e| matches!(e, XaiEvent::Usage { .. })),
            "zero-token usage must not be emitted: {events:?}"
        );
    }

    // ── response.output_item.added: empty call_id rejected ───────────────────

    #[test]
    fn output_item_added_empty_call_id_does_not_register_entry() {
        // `output_item.added` with an empty call_id must be silently rejected by
        // `if !call_id.is_empty()` — no entry is inserted into pending_fc.
        // A subsequent delta for the same empty key finds nothing and is also
        // a no-op (both guards reject empty call_id).
        let added = r#"data: {"type":"response.output_item.added","item":{"type":"function_call","call_id":"","name":""}}"#;
        let delta = r#"data: {"type":"response.function_call_arguments.delta","call_id":"","delta":"partial"}"#;
        let events = parse_lines(&[added, delta]);
        assert!(
            events.is_empty(),
            "added + delta with empty call_id must produce no events: {events:?}"
        );
    }

    // ── response.completed: missing status field — no panic, no events ────────

    #[test]
    fn response_completed_without_status_field_emits_nothing() {
        // A response.completed with no status, no usage, and no response.id
        // (malformed or minimal payload) must not panic and must emit no events.
        // Every extraction path returns None, so no Finished, Usage, or ResponseId
        // is pushed onto the pending queue.
        let line = r#"data: {"type":"response.completed"}"#;
        let events = parse_line_all(line);
        assert!(
            events.is_empty(),
            "response.completed with no fields must emit nothing: {events:?}"
        );
    }

    #[tokio::test]
    async fn parse_sse_response_id_emitted_once_across_multiple_chunks() {
        use futures_util::stream::{self, StreamExt as _};

        // Two chunks both carry an `id` field. ResponseId must be emitted only once.
        let line1 = Bytes::from(
            "data: {\"id\":\"resp_x\",\"type\":\"message.delta\",\"delta\":{\"text\":\"a\"}}\n",
        );
        let line2 = Bytes::from(
            "data: {\"id\":\"resp_x\",\"type\":\"message.delta\",\"delta\":{\"text\":\"b\"}}\n",
        );
        let s = stream::iter(vec![
            Ok::<_, reqwest::Error>(line1),
            Ok::<_, reqwest::Error>(line2),
        ]);
        let events: Vec<_> = parse_sse(s).collect().await;
        let id_count = events
            .iter()
            .filter(|e| matches!(e, XaiEvent::ResponseId { .. }))
            .count();
        assert_eq!(
            id_count, 1,
            "ResponseId must be emitted exactly once per stream, got {id_count}: {events:?}"
        );
    }

    // ── response.done: nested response.id extracted ──────────────────────────

    #[test]
    fn response_done_event_extracts_nested_response_id() {
        // `response.done` shares the match arm with `response.completed` and has
        // the same nested id extraction logic (lines 623-627). When no prior
        // top-level id has been emitted, the nested `response.id` must be
        // extracted and emitted as `ResponseId`. The existing
        // `response_done_event_same_as_completed` test omits the id field, so
        // this extraction path was untested for `response.done`.
        let line = r#"data: {"type":"response.done","response":{"id":"resp-done-123","status":"completed"}}"#;
        let events = parse_lines(&[line]);
        let id_events: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                XaiEvent::ResponseId { id } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            id_events,
            vec!["resp-done-123"],
            "response.done must extract nested response.id when not yet emitted: {events:?}"
        );
    }

    // ── response.cancelled: no ResponseId extraction ──────────────────────────

    #[test]
    fn response_cancelled_does_not_extract_response_id() {
        // Unlike `response.completed` (which extracts the nested `response.id`
        // as a fallback), the `response.cancelled` handler only emits
        // `Finished(Cancelled)`. A ResponseId is NOT extracted even when
        // `response.id` is present in the payload — documenting the intentional
        // asymmetry between the two handlers.
        let line = r#"data: {"type":"response.cancelled","response":{"id":"resp-cancelled","status":"cancelled"}}"#;
        let events = parse_line_all(line);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, XaiEvent::ResponseId { .. })),
            "response.cancelled must not emit ResponseId: {events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(e, XaiEvent::Finished { reason, .. } if *reason == FinishReason::Cancelled)),
            "response.cancelled must emit Finished(Cancelled): {events:?}"
        );
    }

    // ── Message constructors ──────────────────────────────────────────────────

    #[test]
    fn message_user_constructor_sets_role_and_content() {
        let msg = Message::user("hello");
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content, Some("hello".to_string()));
    }

    #[test]
    fn message_assistant_text_constructor_sets_role_and_content() {
        let msg = Message::assistant_text("reply");
        assert_eq!(msg.role, "assistant");
        assert_eq!(msg.content, Some("reply".to_string()));
    }

    // ── Message::content_str ──────────────────────────────────────────────────

    #[test]
    fn message_content_str_some_returns_text() {
        let msg = Message::user("hello");
        assert_eq!(msg.content_str(), "hello");
    }

    #[test]
    fn message_content_str_none_returns_empty_str() {
        let msg = Message {
            role: "user".to_string(),
            content: None,
            prompt_tokens: None,
            completion_tokens: None,
        };
        assert_eq!(msg.content_str(), "");
    }

    // ── XaiClient constructors ────────────────────────────────────────────────

    static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    fn env_lock() -> &'static std::sync::Mutex<()> {
        ENV_LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    #[test]
    fn xai_client_new_uses_default_base_url_when_env_not_set() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::remove_var("XAI_BASE_URL") };
        let client = XaiClient::new();
        assert_eq!(client.base_url, "https://api.x.ai/v1");
    }

    #[test]
    fn xai_client_new_reads_base_url_from_env() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::set_var("XAI_BASE_URL", "https://custom.example.com/v2") };
        let client = XaiClient::new();
        unsafe { std::env::remove_var("XAI_BASE_URL") };
        assert_eq!(client.base_url, "https://custom.example.com/v2");
    }

    #[test]
    fn xai_client_new_uses_default_timeout_when_env_not_set() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS") };
        let client = XaiClient::new();
        assert_eq!(client.request_timeout, Duration::from_secs(300));
    }

    #[test]
    fn xai_client_new_reads_timeout_from_env() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::set_var("XAI_PROMPT_TIMEOUT_SECS", "60") };
        let client = XaiClient::new();
        unsafe { std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS") };
        assert_eq!(client.request_timeout, Duration::from_secs(60));
    }

    #[test]
    fn xai_client_with_base_url_sets_url_and_default_timeout() {
        let client = XaiClient::with_base_url("https://proxy.internal/v1");
        assert_eq!(client.base_url, "https://proxy.internal/v1");
        assert_eq!(client.request_timeout, Duration::from_secs(300));
    }

    #[test]
    fn xai_client_new_timeout_zero_falls_back_to_default() {
        let _guard = env_lock().lock().unwrap();
        // `.filter(|&n| n > 0)` rejects 0, so the default 300 s must be used.
        unsafe { std::env::set_var("XAI_PROMPT_TIMEOUT_SECS", "0") };
        let client = XaiClient::new();
        unsafe { std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS") };
        assert_eq!(client.request_timeout, Duration::from_secs(300));
    }

    // ── start_request: HTTP error status codes ────────────────────────────────
    //
    // These tests spin up a real axum server on a random port so the actual
    // reqwest/HTTP stack is exercised end-to-end. Each test verifies that a
    // non-2xx response from the xAI API is converted to a single
    // `XaiEvent::Error` carrying the status code in its message.

    /// Bind a random local port and return (url, axum server future).
    /// The caller must spawn / drive the server future.
    #[cfg(test)]
    async fn make_test_server(
        app: axum::Router,
    ) -> (String, impl std::future::Future<Output = ()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let url = format!("http://{addr}");
        let server = async move {
            axum::serve(listener, app).await.ok();
        };
        (url, server)
    }

    #[tokio::test]
    async fn chat_stream_on_429_emits_error_event_with_status() {
        use axum::{Router, http::StatusCode, routing::post};
        use futures_util::StreamExt as _;

        let app = Router::new().route(
            "/responses",
            post(|| async { (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded") }),
        );
        let (url, server) = make_test_server(app).await;
        let server_task = tokio::spawn(server);

        let client = XaiClient::with_base_url(url);
        let input = [InputItem::user("hi")];
        let mut stream =
            XaiHttpClient::chat_stream(&client, "grok-3", &input, "key", &[], None, None).await;
        let event = stream.next().await.expect("stream must emit one event");

        server_task.abort();

        match event {
            XaiEvent::Error { message } => {
                assert!(
                    message.contains("429"),
                    "error message must contain HTTP status 429; got: {message}"
                );
            }
            other => panic!("expected XaiEvent::Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn chat_stream_on_500_emits_error_event_with_status() {
        use axum::{Router, http::StatusCode, routing::post};
        use futures_util::StreamExt as _;

        let app = Router::new().route(
            "/responses",
            post(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "internal error") }),
        );
        let (url, server) = make_test_server(app).await;
        let server_task = tokio::spawn(server);

        let client = XaiClient::with_base_url(url);
        let input = [InputItem::user("hi")];
        let mut stream =
            XaiHttpClient::chat_stream(&client, "grok-3", &input, "key", &[], None, None).await;
        let event = stream.next().await.expect("stream must emit one event");

        server_task.abort();

        match event {
            XaiEvent::Error { message } => {
                assert!(
                    message.contains("500"),
                    "error message must contain HTTP status 500; got: {message}"
                );
            }
            other => panic!("expected XaiEvent::Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn chat_stream_on_connection_refused_emits_error_event() {
        // Point at a port where nothing is listening — reqwest connection error
        // must be surfaced as XaiEvent::Error (not a panic or hang).
        use futures_util::StreamExt as _;

        // Bind then immediately drop to guarantee the port is not in use.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let url = format!("http://127.0.0.1:{port}");

        let client = XaiClient::with_base_url(url);
        let input = [InputItem::user("hi")];
        let mut stream =
            XaiHttpClient::chat_stream(&client, "grok-3", &input, "key", &[], None, None).await;
        let event = stream
            .next()
            .await
            .expect("stream must emit one event on connection error");

        assert!(
            matches!(event, XaiEvent::Error { .. }),
            "connection refused must produce XaiEvent::Error, got {event:?}"
        );
    }

    #[tokio::test]
    async fn chat_stream_network_drop_mid_chunk_emits_error_event() {
        // Exercises the `Some(Err(e))` branch inside `parse_sse` (client.rs:361-363).
        //
        // The server sends a valid HTTP 200 with chunked encoding, announces a
        // 32-byte chunk, writes only 13 bytes, then drops the connection. hyper
        // detects the incomplete chunk and surfaces it as an Err on the byte
        // stream, which parse_sse converts to XaiEvent::Error.
        use futures_util::StreamExt as _;
        use tokio::io::AsyncWriteExt as _;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            // Valid 200 response with chunked transfer encoding.
            tcp.write_all(
                b"HTTP/1.1 200 OK\r\n\
                  content-type: text/event-stream\r\n\
                  transfer-encoding: chunked\r\n\
                  \r\n",
            )
            .await
            .ok();
            // Announce a 32-byte (0x20) chunk but only send 13 bytes, then close.
            tcp.write_all(b"20\r\ndata: partial").await.ok();
            tcp.flush().await.ok();
            // Drop closes the TCP connection — hyper will detect the truncated chunk.
        });

        let client = XaiClient::with_base_url(format!("http://{addr}"));
        let input = [InputItem::user("hi")];
        let events: Vec<XaiEvent> =
            XaiHttpClient::chat_stream(&client, "grok-3", &input, "key", &[], None, None)
                .await
                .collect()
                .await;

        assert!(
            events.iter().any(|e| matches!(e, XaiEvent::Error { .. })),
            "mid-stream connection drop must produce XaiEvent::Error; got: {events:?}"
        );
    }
}
