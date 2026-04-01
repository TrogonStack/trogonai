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

use bytes::Bytes;
use futures_util::stream::StreamExt as _;
use futures_util::{Stream, stream};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

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
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self { role: "user".to_string(), content: Some(text.into()) }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self { role: "system".to_string(), content: Some(text.into()) }
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self { role: "assistant".to_string(), content: Some(text.into()) }
    }

    /// Returns the text content of this message, or `""` if none.
    pub fn content_str(&self) -> &str {
        self.content.as_deref().unwrap_or("")
    }
}

/// An item in the `input` array sent to the Responses API.
///
/// Only text roles are needed: tool execution is server-side and never recorded
/// as history entries that need to be replayed in the input array.
#[derive(Serialize, Clone, Debug)]
pub struct InputItem {
    pub role: String,
    pub content: String,
}

impl InputItem {
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user".to_string(), content: content.into() }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self { role: "system".to_string(), content: content.into() }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: "assistant".to_string(), content: content.into() }
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
    ///
    /// `incomplete_reason` is set when `reason == Incomplete` and carries the
    /// value of `incomplete_details.reason` from the API (e.g. `"max_output_tokens"`
    /// or `"max_turns"`). Used by the agent to choose the right continuation strategy.
    Finished { reason: FinishReason, incomplete_reason: Option<String> },
    /// A server-side tool call (web_search, x_search, code_interpreter, file_search)
    /// finished on xAI's infrastructure. Emitted from `response.*_call.completed`
    /// for all four built-in tool types.
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

        // max_turns is only meaningful when tools are present — the server uses
        // it to cap tool-calling iterations. Skip it for tool-less requests to
        // avoid sending a parameter that has no effect.
        if !tools.is_empty() {
            if let Some(turns) = max_turns {
                body["max_turns"] = serde_json::Value::Number(turns.into());
            }
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
                    process_sse_line(&line, &mut state.response_id_emitted, &mut state.pending, &mut state.pending_fc);
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
                            process_sse_line(line, &mut state.response_id_emitted, &mut state.pending, &mut state.pending_fc);
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
/// - `response.web_search_call.completed` / `response.x_search_call.completed` /
///   `response.code_interpreter_call.completed` / `response.file_search_call.completed`
///   → `ServerToolCompleted` (tool finished; agent advances tool to Completed)
/// - `*.in_progress` / `*.searching` / `*.interpreting` variants for all four
///   built-in tools → explicit no-op (informational; no state change)
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
        // Reasoning models (e.g. grok-3-mini) may also include a
        // "reasoning_content" field in the same delta object; it is logged and
        // discarded since the ACP protocol has no dedicated reasoning block type.
        "message.delta" | "response.output_text.delta" => {
            if let Some(text) = val["delta"]["text"].as_str()
                .or_else(|| val["delta"].as_str())
            {
                if !text.is_empty() {
                    pending.push_back(XaiEvent::TextDelta { text: text.to_string() });
                }
            }
            if let Some(rc) = val["delta"]["reasoning_content"].as_str() {
                if !rc.is_empty() {
                    debug!(reasoning_len = rc.len(), "xai: reasoning_content chunk (not forwarded to client)");
                }
            }
        }
        // OpenAI Responses API reasoning summary events — log and discard.
        "response.reasoning_summary_text.delta" => {
            let rc = val["delta"].as_str().unwrap_or("");
            if !rc.is_empty() {
                debug!(reasoning_len = rc.len(), "xai: reasoning summary chunk (not forwarded to client)");
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
                pending.push_back(XaiEvent::FunctionCall { call_id, name, arguments });
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
            if !call_id.is_empty() && !delta.is_empty() {
                if let Some(entry) = pending_fc.get_mut(call_id) {
                    entry.1.push_str(delta);
                }
            }
        }
        "response.function_call_arguments.done" => {
            // Complete function call — arguments are authoritative here.
            // Name falls back to what was recorded in output_item.added.
            let call_id = val["call_id"].as_str().unwrap_or("").to_string();
            let arguments = val["arguments"].as_str().unwrap_or("").to_string();
            let name = val["name"].as_str()
                .map(str::to_string)
                .or_else(|| pending_fc.get(&call_id).map(|(n, _)| n.clone()))
                .unwrap_or_default();
            pending_fc.remove(&call_id);
            if !call_id.is_empty() || !name.is_empty() {
                pending.push_back(XaiEvent::FunctionCall { call_id, name, arguments });
            }
        }
        // ── Server-side tool call lifecycle events ───────────────────────────
        // xAI emits these while its infrastructure executes a built-in tool
        // (web_search, x_search, code_interpreter, file_search). The `.completed` event
        // fires as soon as the search finishes — before the model begins
        // streaming its text answer. Emit ServerToolCompleted so the agent
        // can advance the tool call to Completed mid-stream rather than
        // waiting for [DONE] (which may arrive 10–20 s later after text gen).
        //
        // The in_progress and searching events are purely informational; they
        // carry no actionable data for the agent so they are dropped explicitly
        // (preferred over falling through to `_ => {}` for clarity).
        "response.web_search_call.completed" => {
            pending.push_back(XaiEvent::ServerToolCompleted { name: "web_search".to_string() });
        }
        "response.x_search_call.completed" => {
            pending.push_back(XaiEvent::ServerToolCompleted { name: "x_search".to_string() });
        }
        "response.code_interpreter_call.completed" => {
            pending.push_back(XaiEvent::ServerToolCompleted { name: "code_interpreter".to_string() });
        }
        "response.file_search_call.completed" => {
            pending.push_back(XaiEvent::ServerToolCompleted { name: "file_search".to_string() });
        }
        "response.web_search_call.in_progress"
        | "response.web_search_call.searching"
        | "response.x_search_call.in_progress"
        | "response.x_search_call.searching"
        | "response.code_interpreter_call.in_progress"
        | "response.code_interpreter_call.interpreting"
        | "response.file_search_call.in_progress"
        | "response.file_search_call.searching" => {}
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
            if !*response_id_emitted {
                if let Some(id) = val["response"]["id"].as_str() {
                    pending.push_back(XaiEvent::ResponseId { id: id.to_string() });
                    *response_id_emitted = true;
                }
            }
            let usage = if val["usage"].is_null() { &val["response"]["usage"] } else { &val["usage"] };
            let p = usage["prompt_tokens"].as_u64().unwrap_or(0);
            let c = usage["completion_tokens"].as_u64().unwrap_or(0);
            if p > 0 || c > 0 {
                pending.push_back(XaiEvent::Usage { prompt_tokens: p, completion_tokens: c });
            }
            let incomplete_reason = val["response"]["incomplete_details"]["reason"].as_str()
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
            let message = val["error"]["message"].as_str()
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
            if !*response_id_emitted {
                if let Some(id) = val["response"]["id"].as_str() {
                    pending.push_back(XaiEvent::ResponseId { id: id.to_string() });
                    *response_id_emitted = true;
                }
            }
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
            // When status is "incomplete", also extract incomplete_details.reason
            // (e.g. "max_output_tokens" or "max_turns") so the agent can choose
            // the right continuation strategy.
            let status = val["response"]["status"].as_str()
                .or_else(|| val["status"].as_str());
            if let Some(s) = status {
                let incomplete_reason = if s == "incomplete" {
                    val["response"]["incomplete_details"]["reason"].as_str()
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
        let mut pending_fc = HashMap::new();
        process_sse_line(line, &mut emitted, &mut pending, &mut pending_fc);
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
            XaiEvent::FunctionCall { call_id, name, arguments } => {
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
        assert_eq!(events.len(), 1, "expected exactly one FunctionCall: {events:?}");
        match &events[0] {
            XaiEvent::FunctionCall { call_id, name, arguments } => {
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
        let line = r#"data: {"type":"response.output_item.added","item":{"type":"message","id":"msg_1"}}"#;
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
        let id_events: Vec<_> = events.iter().filter_map(|e| match e {
            XaiEvent::ResponseId { id } => Some(id.as_str()),
            _ => None,
        }).collect();
        assert_eq!(id_events, vec!["resp_trunc"], "response.incomplete must emit ResponseId");
        let usage_event = events.iter().find(|e| matches!(e, XaiEvent::Usage { .. }));
        assert!(usage_event.is_some(), "response.incomplete must emit Usage");
        let finished = events.iter().find_map(|e| match e {
            XaiEvent::Finished { reason, incomplete_reason } => Some((reason.clone(), incomplete_reason.clone())),
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
            XaiEvent::Finished { reason, incomplete_reason } => Some((reason.clone(), incomplete_reason.clone())),
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
                assert!(message.contains("content policy"), "unexpected message: {message}");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn response_error_event_fallback_message() {
        // Fallback when error object is absent or oddly shaped.
        let line = r#"data: {"type":"response.error","message":"unexpected failure"}"#;
        let event = parse_line(line).unwrap();
        assert!(matches!(event, XaiEvent::Error { .. }), "expected Error, got {event:?}");
    }

    #[test]
    fn response_completed_nested_id_emitted_when_not_yet_captured() {
        // When response.completed is the first (or only) event and carries no
        // top-level id, the nested response.id must be extracted so the agent
        // can use previous_response_id on the next turn.
        let line = r#"data: {"type":"response.completed","response":{"id":"resp_nested","status":"completed"}}"#;
        let events = parse_lines(&[line]);
        let id_events: Vec<_> = events.iter().filter_map(|e| match e {
            XaiEvent::ResponseId { id } => Some(id.as_str()),
            _ => None,
        }).collect();
        assert_eq!(id_events, vec!["resp_nested"], "nested response.id must be emitted as ResponseId");
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
        let id_events: Vec<_> = events.iter().filter_map(|e| match e {
            XaiEvent::ResponseId { id } => Some(id.as_str()),
            _ => None,
        }).collect();
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

    #[test]
    fn code_interpreter_call_completed_emits_search_call_completed() {
        let line = r#"data: {"type":"response.code_interpreter_call.completed","item_id":"ci_abc","output_index":0}"#;
        let event = parse_line(line).unwrap();
        assert!(
            matches!(event, XaiEvent::ServerToolCompleted { ref name } if name == "code_interpreter"),
            "expected ServerToolCompleted(code_interpreter), got {event:?}"
        );
    }

    #[test]
    fn code_interpreter_call_in_progress_is_silent() {
        let line = r#"data: {"type":"response.code_interpreter_call.in_progress","item_id":"ci_abc","output_index":0}"#;
        assert!(parse_line(line).is_none(), "code_interpreter_call.in_progress must produce no event");
    }

    #[test]
    fn code_interpreter_call_interpreting_is_silent() {
        let line = r#"data: {"type":"response.code_interpreter_call.interpreting","item_id":"ci_abc","output_index":0}"#;
        assert!(parse_line(line).is_none(), "code_interpreter_call.interpreting must produce no event");
    }

    #[test]
    fn file_search_call_completed_emits_search_call_completed() {
        let line = r#"data: {"type":"response.file_search_call.completed","item_id":"fs_abc","output_index":0}"#;
        let event = parse_line(line).unwrap();
        assert!(
            matches!(event, XaiEvent::ServerToolCompleted { ref name } if name == "file_search"),
            "expected ServerToolCompleted(file_search), got {event:?}"
        );
    }

    #[test]
    fn file_search_call_in_progress_is_silent() {
        let line = r#"data: {"type":"response.file_search_call.in_progress","item_id":"fs_abc","output_index":0}"#;
        assert!(parse_line(line).is_none(), "file_search_call.in_progress must produce no event");
    }

    #[test]
    fn file_search_call_searching_is_silent() {
        let line = r#"data: {"type":"response.file_search_call.searching","item_id":"fs_abc","output_index":0}"#;
        assert!(parse_line(line).is_none(), "file_search_call.searching must produce no event");
    }
}
