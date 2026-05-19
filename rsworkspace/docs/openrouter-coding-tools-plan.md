# openrouter-runner — coding tools implementation plan

Adds full programming tool support to `trogon-openrouter-runner` using OpenRouter's
native OpenAI-compatible `/chat/completions` API with function calling.

**Approach:** extend the existing client with OpenAI function calling — same pattern
as xai-runner but adapted to OpenAI format. No dependency on `trogon-agent-core` or
OpenRouter's Anthropic translation layer.

**Confirmed:** OpenRouter supports OpenAI function calling for all models (GPT, Claude,
Gemini, Llama, etc.) via its primary `/chat/completions` endpoint.

---

## Files changed

| File | Change |
|------|--------|
| `Cargo.toml` | Add `trogon-tools` |
| `src/client.rs` | New types, SSE parser extension |
| `src/http_client.rs` | Updated trait signature + mock |
| `src/agent.rs` | Session struct, tool init, tool loop |

Total estimated: ~470 lines including tests.

---

## Step 1 — Cargo.toml

```toml
trogon-tools        = { path = "../trogon-tools" }
trogon-runner-tools = { path = "../trogon-runner-tools" }
```

`trogon-runner-tools` provides `egress::EgressPolicy` for `fetch_url` URL blocking.
Required in v1 — see Step 17.

---

## Step 2 — client.rs: new types

### ToolDef
Definition of a tool sent to OpenRouter in OpenAI format:

```rust
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}
```

Built from `trogon_tools::all_tool_defs()` in the agent.

### AssembledToolCall
A fully reassembled tool call after fragment accumulation:

```rust
pub struct AssembledToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,  // raw JSON string, e.g. {"path": "/foo"}
}
```

### New OpenRouterEvent variant

```rust
pub enum OpenRouterEvent {
    TextDelta { text: String },
    ToolCallsReady { calls: Vec<AssembledToolCall> },  // NEW
    Usage { prompt_tokens: u64, completion_tokens: u64 },
    Finished { reason: FinishReason },
    Done,
    Error { message: String },
}
```

---

## Step 3 — client.rs: SSE parser extension

OpenAI streams tool calls as fragments across multiple chunks. Each chunk carries
a partial `delta.tool_calls` array with an `index` field. The parser must accumulate
by index and emit `ToolCallsReady` when `finish_reason == "tool_calls"` arrives.

### Example wire format

```
// chunk 1 — header (id + name)
data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_xyz","type":"function","function":{"name":"read_file","arguments":""}}]},"finish_reason":null}]}

// chunk 2 — arguments fragment
data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]},"finish_reason":null}]}

// chunk 3 — arguments fragment
data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"/foo\"}"}}]},"finish_reason":null}]}

// chunk 4 — finish
data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}
```

### SseState — add accumulator

```rust
struct SseState {
    stream: LocalBoxStream<'static, Result<Bytes, reqwest::Error>>,
    buf: String,
    pending: VecDeque<OpenRouterEvent>,
    tool_call_acc: HashMap<usize, PartialToolCall>,  // NEW
}

struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}
```

### process_sse_line — extended logic

```
if delta.tool_calls present:
    for each entry in delta.tool_calls:
        acc[index].id    = merge(existing, entry.id)
        acc[index].name  = merge(existing, entry.function.name)
        acc[index].arguments += entry.function.arguments  // concatenate

if finish_reason == "tool_calls":
    assemble acc into Vec<AssembledToolCall>
    emit ToolCallsReady { calls }
    clear acc

if finish_reason == "stop" / "length" / other:
    emit Finished { reason }  // unchanged
```

Tool call fragment assembly happens before `Finished` is emitted — the parser
must pass the accumulator through `SseState` so fragments from different lines
are merged correctly.

### process_sse_line signature change

The accumulator lives in `SseState`. The cleanest approach is to pass it explicitly:

```rust
fn process_sse_line(
    line: &str,
    pending: &mut VecDeque<OpenRouterEvent>,
    acc: &mut HashMap<usize, PartialToolCall>,  // NEW
)
```

This changes the signature of `process_sse_line`, which is called directly by the
`events_from_lines` test helper in the existing unit tests. Update `events_from_lines`
to create and pass an empty accumulator:

```rust
fn events_from_lines(lines: &[&str]) -> Vec<OpenRouterEvent> {
    let mut pending = VecDeque::new();
    let mut acc = HashMap::new();   // NEW
    for line in lines {
        process_sse_line(line, &mut pending, &mut acc);
    }
    pending.into_iter().collect()
}
```

All existing parser unit tests continue to work unchanged via `events_from_lines`.

---

## Step 4 — http_client.rs: updated trait and mock

### Trait

```rust
#[async_trait(?Send)]
pub trait OpenRouterHttpClient {
    async fn chat_stream(
        &self,
        model: &str,
        messages: &[Message],
        api_key: &str,
        tools: &[ToolDef],    // NEW
    ) -> LocalBoxStream<'static, OpenRouterEvent>;
}
```

### Breaking change — all trait implementors must be updated simultaneously

Adding `tools: &[ToolDef]` to the trait signature is a compile-time breaking change.
All 7 implementations must be updated in the same commit or the build will not compile:

| File | Implementor |
|------|-------------|
| `src/client.rs` | `OpenRouterClient` |
| `src/http_client.rs` | `MockOpenRouterHttpClient` |
| `src/http_client.rs` | `Arc<MockOpenRouterHttpClient>` |
| `tests/nats_e2e.rs` | `NoOpHttpClient` |
| `tests/agent_nats_integration.rs` | `NoOpHttpClient` |
| `tests/agent_nats_integration.rs` | `ReplyHttpClient` |
| `tests/e2e_mock.rs` | `TestHttpClient` |

The test-file implementations (`NoOpHttpClient`, `ReplyHttpClient`, `TestHttpClient`)
receive `tools` but can ignore it — they just need the parameter in the signature.

### Direct callers of chat_stream in tests/live_http.rs

`tests/live_http.rs` calls `chat_stream` directly on a real `OpenRouterClient`
(not via the agent). These 4 call sites also need the new `tools` argument:

| Line | Call site |
|------|-----------|
| 211 | `.chat_stream(model, &[Message::user("hello")], api_key)` |
| 332 | `.chat_stream("anthropic/claude-sonnet-4-6", &messages, "sk-test")` |
| 359 | `.chat_stream("test-model", &messages, "sk-test")` |
| 658 | `.chat_stream("test-model", &[Message::user("hello")], "sk-test")` |

Each becomes `.chat_stream(model, messages, api_key, &[])`. These are not trait
implementors — they are call sites, but they still break at compile time.

### start_request — add tools to body

```rust
let mut body = serde_json::json!({
    "model": model,
    "messages": wire_messages,
    "stream": true,
    "stream_options": { "include_usage": true },
});

if !tools.is_empty() {
    body["tools"] = serde_json::json!(
        tools.iter().map(|t| serde_json::json!({
            "type": "function",
            "function": {
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
            }
        })).collect::<Vec<_>>()
    );
}
```

### Mock

Add `tools: Vec<ToolDef>` to `MockCall` so tests can assert which tools were sent.

---

## Step 5 — agent.rs: Message struct extension

The current `Message` struct only handles text. Tool support requires two additional
wire shapes:

**Assistant message with tool calls** (stored after model responds with tools):
```json
{
  "role": "assistant",
  "content": null,
  "tool_calls": [{"id": "call_xyz", "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"/foo\"}"}}]
}
```

**Tool result message** (sent back with tool output):
```json
{"role": "tool", "tool_call_id": "call_xyz", "content": "line 1: ..."}
```

### Changes to Message

Add optional fields (additive — existing code unaffected):

```rust
pub struct Message {
    pub role: String,
    pub content: String,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub tool_calls: Option<Vec<ToolCallMessage>>,  // NEW: assistant tool call list
    pub tool_call_id: Option<String>,              // NEW: tool result id
}

pub struct ToolCallMessage {
    pub id: String,
    pub name: String,
    pub arguments: String,
}
```

New constructors:

```rust
impl Message {
    pub fn assistant_tool_calls(calls: &[AssembledToolCall]) -> Self { ... }
    pub fn tool_result(tool_call_id: String, content: String) -> Self { ... }
}
```

The `wire_messages` serialization in `start_request` already uses explicit
`serde_json::json!` — extend it to emit `tool_calls` / `tool_call_id` when present.

---

## Step 6 — agent.rs: session struct and initialization

### OpenRouterSession — add enabled_tools

```rust
struct OpenRouterSession {
    cwd: String,
    model: Option<String>,
    api_key: Option<String>,
    history: Vec<Message>,
    system_prompt: Option<String>,
    enabled_tools: Vec<String>,        // NEW — all tools on by default
    created_at: Instant,
    created_at_iso: String,
    parent_session_id: Option<String>,
    branched_at_index: Option<usize>,
}
```

### OpenRouterAgent — add tool_http_client

```rust
pub struct OpenRouterAgent<H = OpenRouterClient, N = NatsSessionNotifier> {
    // ... existing fields ...
    tool_http_client: reqwest::Client,   // NEW — used in ToolContext for fetch_url etc.
}
```

Initialize in constructor:
```rust
tool_http_client: reqwest::Client::new(),
```

### new_session — initialize tools

```rust
enabled_tools: trogon_tools::all_tool_defs()
    .iter()
    .map(|t| t.name.clone())
    .collect(),
```

Return config options so the console can toggle individual tools:

```rust
Ok(NewSessionResponse::new(SessionId::from(session_id))
    .modes(self.session_mode_state())
    .models(self.session_model_state(None))
    .config_options(Self::all_tool_config_options(&enabled_tools)))
```

### Tool config option helpers (same pattern as xai-runner)

```rust
fn all_tool_config_options(enabled_tools: &[String]) -> Vec<SessionConfigOption> { ... }
```

One `SessionConfigOption::select(...)` entry per tool. Toggling calls
`set_session_config_option` which pushes/removes from `enabled_tools`.

---

## Step 7 — agent.rs: tool execution loop

### Extract cwd and enabled_tools from session

In `prompt`, extend the session snapshot tuple:

```rust
let (model, api_key, mut messages, session_system_prompt, enabled_tools, cwd) = { ... };
```

### Build tool defs for request

```rust
let tool_defs: Vec<ToolDef> = trogon_tools::all_tool_defs()
    .into_iter()
    .filter(|d| enabled_tools.contains(&d.name))
    .map(|d| ToolDef {
        name: d.name,
        description: d.description,
        parameters: d.input_schema,
    })
    .collect();
```

### Outer loop structure

```rust
let mut tool_rounds: u32 = 0;
const MAX_TOOL_ROUNDS: u32 = 10;

'outer: loop {
    // Reset per-round accumulators at the start of each iteration.
    // assistant_text must be cleared so intermediate tool-round text
    // does not bleed into the final assistant message.
    assistant_text.clear();
    usage = None;

    // Create a new stream each iteration, with cancel check on creation.
    let stream_fut = client.chat_stream(&model, &wire_messages, &api_key, &tool_defs);
    let mut stream = tokio::select! {
        s = stream_fut => s,
        _ = &mut cancel_rx => {
            canceled = true;
            futures_util::stream::empty().boxed_local()
        }
    };

    let mut assembled_calls: Vec<AssembledToolCall> = Vec::new();

    // inner event loop — same cancel check pattern as today
    loop {
        if canceled { break; }

        let event = tokio::select! {
            result = stream.next() => match result {
                Some(ev) => ev,
                None => break,
            },
            _ = &mut cancel_rx => {
                canceled = true;
                break;
            }
        };

        match event {
            TextDelta { text } => { /* notify + accumulate — unchanged */ }

            ToolCallsReady { calls } => {
                assembled_calls = calls;
                break; // exit inner loop → go execute tools
            }

            Finished { reason: FinishReason::Stop | FinishReason::Length } => {
                break 'outer StopReason::EndTurn;
            }

            Done => break 'outer StopReason::EndTurn,
            Error { .. } => { /* log, break */ }
        }
    }

    // If canceled during the inner loop or stream creation, stop immediately.
    if canceled {
        break StopReason::Cancelled;
    }

    if assembled_calls.is_empty() {
        break StopReason::EndTurn;
    }

    if tool_rounds >= MAX_TOOL_ROUNDS {
        warn!(session_id, "openrouter: max tool rounds reached");
        break StopReason::Cancelled;
    }

    // store assistant tool_calls message in wire_messages and history
    let tool_calls_msg = Message::assistant_tool_calls(&assembled_calls);
    messages.push(tool_calls_msg.clone());
    wire_messages.push(tool_calls_msg);

    let ctx = trogon_tools::ToolContext {
        proxy_url: String::new(),
        cwd: cwd.clone(),
        http_client: self.tool_http_client.clone(),
    };

    for call in &assembled_calls {
        let kind = ToolKind::Other; // openrouter-runner has no bash support in v1
        notifier.notify(ToolCall(InProgress, &call.id, &call.name, kind)).await;

        let input = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
        let result = trogon_tools::dispatch_tool(&ctx, &call.name, &input).await;

        notifier.notify(ToolCallUpdate(Completed, &call.id, &result)).await;

        let result_msg = Message::tool_result(call.id.clone(), result);
        messages.push(result_msg.clone());
        wire_messages.push(result_msg);
    }

    tool_rounds += 1;
    // continue 'outer → send next request with tool results appended
}
```

### Inner loop — behaviours that must be preserved

The current inner loop has four behaviours that are not in the pseudo-code above
and must be preserved in every iteration of the outer loop:

**1. Per-chunk timeout**
```rust
let next = tokio::time::timeout(self.prompt_timeout, stream.next());
```
Without this, a stalled stream blocks forever. Wrap `stream.next()` with the timeout
in each inner iteration, same as today.

**2. max_response_bytes guard**
```rust
if assistant_text.len() > self.max_response_bytes {
    warn!(...);
    break; // break inner loop → assembled_calls is empty → EndTurn
}
```
Still applies in the final round.

**3. UsageUpdate notification**
```rust
OpenRouterEvent::Usage { prompt_tokens, completion_tokens } => {
    usage = Some((prompt_tokens, completion_tokens));
    notifier.notify(SessionUpdate::UsageUpdate(...)).await;
}
```
Usage arrives in the final round. The notification must still be sent.

**4. drop(stream) before any awaits after the inner loop**
```rust
drop(stream);
```
Closes the HTTP connection immediately on cancellation, stopping token generation.
Must happen after the inner loop exits, before tool dispatch or history writes.

### Cleanup and history persistence after outer loop

After the outer loop exits (EndTurn or Cancelled):

```rust
// 1. Drop the last stream (connection close)
drop(stream);  // stream from last outer iteration

// 2. Remove cancel sender
self.cancel_senders.lock().await.remove(&session_id);

// 3. Persist history
//
// `messages` already contains: old history + user message (if !resuming) +
// all tool rounds. Only the final assistant text remains to be appended.
{
    let mut sessions = self.sessions.lock().await;
    if let Some(s) = sessions.get_mut(&session_id) {
        if !assistant_text.is_empty() {
            let msg = if let Some((pt, ct)) = usage {
                Message::assistant_with_usage(&assistant_text, pt, ct)
            } else {
                Message::assistant(&assistant_text)
            };
            messages.push(msg);
        }
        s.history = messages;
        Self::trim_history(&mut s.history, self.max_history);
        if let Some(store) = &self.session_store {
            let snapshot = self.build_snapshot(&session_id, s);
            store.save(&snapshot).await;
        }
    }
}

// 4. stop_reason comes from the outer loop break value (already StopReason)
Ok(PromptResponse::new(stop_reason))
```

### History persistence

`wire_messages` starts with the system prompt (index 0, not stored in history). To
avoid re-introducing it, maintain `messages` as the history working copy and append
tool messages directly to it during the loop — not just to `wire_messages`:

```rust
// When tool calls are ready:
let tool_calls_msg = Message::assistant_tool_calls(&assembled_calls);
messages.push(tool_calls_msg.clone());       // → history
wire_messages.push(tool_calls_msg);          // → next request

// After each tool is executed:
let result_msg = Message::tool_result(call.id.clone(), result);
messages.push(result_msg.clone());           // → history
wire_messages.push(result_msg);              // → next request
```

At the end of the outer loop, `messages` already contains the complete history
(user message + all tool rounds + final assistant text). Store it directly:

```rust
sessions.get_mut(&session_id).unwrap().history = messages;
```

This avoids slicing `wire_messages` and the system prompt exclusion problem.

### Bash support

Implemented in Steps 15–16. `execute_bash_via_nats` is wired via `with_execution_backend`
in `main.rs`. Bash is available when an execution backend with `agent_type: "execution"`
is registered in the NATS registry; absent that, it degrades silently.

---

## Step 8 — Tests

All tests use `MockOpenRouterHttpClient`. Follow the existing patterns in `agent.rs`
`#[cfg(test)]` block.

| Test | What it verifies |
|------|-----------------|
| `new_session_has_all_tool_defs_in_config_options` | All trogon-tools appear as config options with enabled=true |
| `tool_defs_sent_in_request_when_enabled` | `MockCall.tools` contains the expected tool defs |
| `disabled_tool_not_sent_in_request` | After toggling a tool off, it is absent from the next request |
| `tool_call_dispatched_and_follow_up_sent` | Mock returns `ToolCallsReady` + `Finished(stop)`; second `MockCall` contains `role: "tool"` message |
| `tool_call_notifies_in_progress_and_completed` | `ToolCall(InProgress)` and `ToolCallUpdate(Completed)` are emitted |
| `tool_result_stored_in_history` | After a tool round, history contains the assistant tool_calls message and tool result message |
| `max_tool_rounds_returns_cancelled` | 10 consecutive `ToolCallsReady` responses → `StopReason::Cancelled` |
| `sse_parser_assembles_single_tool_call_from_fragments` | 3 chunks with fragmented arguments → one `ToolCallsReady` with correct assembled call |
| `sse_parser_assembles_two_parallel_tool_calls` | Two tools in same response, each fragmented → `ToolCallsReady` with 2 calls |

---

## Implementation order

| Step | What | Est. lines |
|------|------|-----------|
| 1 | Cargo.toml | 2 |
| 2–3 | client.rs: types + SSE parser | ~80 |
| 4 | http_client.rs: trait + mock | ~20 |
| 5 | Message extension | ~30 |
| 6 | Session struct + init + config options | ~60 |
| 7 | Tool execution loop in prompt | ~100 |
| 8 | Tests | ~200 |
| 9 | `set_session_config_option` handler | ~50 |
| 10 | `load_session` + `fork_session` config options + tools inheritance | ~20 |
| 11 | `trim_history` tool-round awareness + tests | ~20 |
| 12 | Wire serialization for tool messages in `start_request` | ~20 |
| 13 | `resume_session` returns config options | ~10 |
| 14 | `build_snapshot` skips tool-call intermediate messages | ~10 |
| 15 | agent.rs: bash execution backend | ~60 |
| 16 | main.rs: wire `with_execution_backend` | ~5 |
| 17 | agent.rs: egress policy for fetch_url | ~10 |
| **Total** | | **~675** |

---

## Step 9 — agent.rs: set_session_config_option handler

`set_session_config_option` does not exist in openrouter-runner today — it must be
added as a new ACP method handler (same as xai-runner lines 807-853).

The handler:
1. Reads `req.config_id` (tool name) and `req.value` (enabled/disabled)
2. Pushes or removes from `session.enabled_tools`
3. Returns the full updated `all_tool_config_options` snapshot

```rust
async fn set_session_config_option(
    &self,
    req: SetSessionConfigOptionRequest,
) -> agent_client_protocol::Result<SetSessionConfigOptionResponse> {
    let session_id = req.session_id.to_string();
    let config_id = req.config_id.to_string();
    let mut sessions = self.sessions.lock().await;
    let s = sessions.get_mut(&session_id)
        .ok_or_else(|| not_found(format!("session {session_id} not found")))?;

    match &req.value {
        SessionConfigOptionValue::ValueId { value } if value == "enabled" => {
            if !s.enabled_tools.contains(&config_id) {
                s.enabled_tools.push(config_id.clone());
            }
        }
        SessionConfigOptionValue::ValueId { value } if value == "disabled" => {
            s.enabled_tools.retain(|t| t != &config_id);
        }
        _ => {
            warn!(config_id = %config_id, "openrouter: set_session_config_option unknown option — ignored");
        }
    }

    let config_options = Self::all_tool_config_options(&s.enabled_tools);
    Ok(SetSessionConfigOptionResponse::new(config_options))
}
```

---

## Step 10 — agent.rs: load_session and fork_session

Both currently return `config_options(vec![])`. Must return actual tool config options.

### load_session

```rust
// Before
.config_options(vec![])

// After
.config_options(Self::all_tool_config_options(&s.enabled_tools))
```

Also extract `enabled_tools` from the session snapshot tuple.

### fork_session

`fork_session` does not inherit `enabled_tools` from the parent session today.
Must be added to the inherited tuple and the new session construction:

```rust
let (inherited_model, inherited_key, mut history, inherited_system_prompt, inherited_tools) = {
    ...
    s.enabled_tools.clone(),
    ...
};

// In the new session:
OpenRouterSession {
    ...
    enabled_tools: inherited_tools.clone(),
    ...
}

// In the response:
.config_options(Self::all_tool_config_options(&inherited_tools))
```

---

## Step 11 — agent.rs: trim_history tool-round awareness

The current `trim_history` removes messages one-by-one from the front. With tool
messages in history (assistant tool_calls + one or more tool results), trimming in
the middle of a tool round leaves orphaned `role: "tool"` messages without a
corresponding assistant tool_calls message, which corrupts the conversation.

`trim_history` must always remove complete "turns": a user message, followed by
all assistant + tool messages up to the next user message.

```rust
fn trim_history(history: &mut Vec<Message>, max: usize) {
    while history.len() > max {
        // Always remove the first complete turn: the leading user message
        // plus all messages until (but not including) the next user message.
        history.remove(0); // remove user message
        while history.first().map(|m| m.role != "user").unwrap_or(false) {
            history.remove(0); // remove assistant + tool messages in this turn
        }
    }
}
```

Update the existing `trim_history` unit tests to cover tool message pairs.

---

## Step 12 — client.rs: wire serialization for tool messages

`start_request` currently serializes all messages as:
```rust
serde_json::json!({ "role": m.role, "content": m.content })
```

This must be extended to handle the two new message shapes:

**Assistant message with tool calls** (`tool_calls` is Some):
```json
{
  "role": "assistant",
  "content": null,
  "tool_calls": [
    {"id": "call_xyz", "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"/foo\"}"}}
  ]
}
```

**Tool result message** (`tool_call_id` is Some):
```json
{"role": "tool", "tool_call_id": "call_xyz", "content": "line 1: ..."}
```

The serialization logic in `start_request` should match on which optional fields
are present and emit the appropriate shape. Existing text messages are unchanged.

---

## Step 13 — agent.rs: resume_session returns config options

`resume_session` currently returns `ResumeSessionResponse::new()` with no config
options. After tools are added, the console needs the tool state on resume so it
can show the correct toggle states.

```rust
async fn resume_session(&self, req: ResumeSessionRequest) -> ... {
    let session_id = req.session_id.to_string();
    let sessions = self.sessions.lock().await;
    let s = sessions.get(&session_id)
        .ok_or_else(|| not_found(...))?;
    Ok(ResumeSessionResponse::new()
        .config_options(Self::all_tool_config_options(&s.enabled_tools)))
}
```

---

## Step 14 — build_snapshot: tool message handling

`build_snapshot` maps history to `SnapshotMessage { role, content }`. Tool messages
have empty `content` (assistant tool_calls messages) or carry a `tool_call_id` that
`SnapshotMessage` has no field for.

**Decision for v1:** skip tool-call intermediate messages from the snapshot.
Only text messages (role=user, role=assistant with non-empty content) are included.
Tool result messages (role=tool) and assistant tool_calls messages are excluded.

This means after a server restart, tool call history is not restored into context.
The conversation remains coherent because the final assistant text response (after
tool execution) is always stored as a normal text message and is included.

```rust
let messages = session
    .history
    .iter()
    .filter(|m| m.tool_call_id.is_none() && m.tool_calls.is_none())
    .map(|m| SnapshotMessage { ... })
    .collect();
```

Add a note to the snapshot name derivation: `history.first()` already skips
correctly since the first message is always `role=user` with text content.

---

## Step 15 — agent.rs: bash execution backend

### No new Cargo.toml changes needed

`trogon-registry` and `async-nats` (with `jetstream` + `kv` features) are already
in `trogon-openrouter-runner/Cargo.toml`.

### New fields on OpenRouterAgent

```rust
pub struct OpenRouterAgent<H = OpenRouterClient, N = NatsSessionNotifier> {
    // ... existing fields ...
    registry: Option<Arc<trogon_registry::Registry<async_nats::jetstream::kv::Store>>>,
    execution_nats: Option<async_nats::Client>,
}
```

Initialize both to `None` in `with_deps`.

### New builder method

```rust
pub fn with_execution_backend(
    mut self,
    nats: async_nats::Client,
    registry: trogon_registry::Registry<async_nats::jetstream::kv::Store>,
) -> Self {
    self.execution_nats = Some(nats);
    self.registry = Some(Arc::new(registry));
    self
}
```

### Discover wasm_prefix once per prompt

At the start of `prompt`, after extracting session fields, discover the execution
backend — same pattern as xai-runner:

```rust
let wasm_prefix: Option<String> = if let (Some(reg), Some(_)) =
    (&self.registry, &self.execution_nats)
{
    reg.discover("execution")
        .await
        .ok()
        .and_then(|mut entries| entries.drain(..).next())
        .and_then(|e| {
            e.metadata["acp_prefix"]
                .as_str()
                .map(str::to_string)
                .or_else(|| Some("acp.wasm".to_string()))
        })
} else {
    None
};
```

If no wasm-runtime is registered, `wasm_prefix` is `None` and bash is silently
absent — graceful degradation, same as xai-runner.

### Inject bash into tool_defs

After building `tool_defs` from `trogon_tools::all_tool_defs()` (Step 7), append
bash when available:

```rust
if wasm_prefix.is_some() {
    tool_defs.push(ToolDef {
        name: "bash".to_string(),
        description: "Run a shell command in the session sandbox and return its output.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute."
                }
            },
            "required": ["command"]
        }),
    });
}
```

### Dispatch bash vs trogon-tools

In the tool dispatch loop (Step 7), distinguish bash from the 12 trogon-tools:

```rust
for call in &assembled_calls {
    let kind = if call.name == "bash" { ToolKind::Execute } else { ToolKind::Other };
    notifier.notify(ToolCall(InProgress, &call.id, &call.name, kind)).await;

    let result = if call.name == "bash" {
        if let Some(nats) = &self.execution_nats {
            let wasm = wasm_prefix.as_deref().unwrap_or("acp.wasm");
            execute_bash_via_nats(nats, wasm, &session_id, &call.arguments).await
        } else {
            "bash not available: no execution backend configured".to_string()
        }
    } else {
        let input = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
        trogon_tools::dispatch_tool(&ctx, &call.name, &input).await
    };

    notifier.notify(ToolCallUpdate(Completed, &call.id, &result)).await;
    // push tool_result to messages and wire_messages as before
}
```

### execute_bash_via_nats function

Copy verbatim from `trogon-xai-runner/src/agent.rs` (lines 1520–1600). The
function is self-contained and has no xai-runner-specific dependencies:

```rust
async fn execute_bash_via_nats(
    nats: &async_nats::Client,
    wasm_prefix: &str,
    session_id: &str,
    arguments: &str,
) -> String { ... }
```

Uses four NATS requests: `create → wait_for_exit → output → release`, with a
30-second timeout. Returns the captured stdout/stderr or an error string.

---

## Step 16 — main.rs: wire with_execution_backend

`main.rs` already provisions a registry and has a `nats` client. Add one call
after the existing agent builder chain:

```rust
agent = agent.with_execution_backend(nats.clone(), registry_for_agent);
```

The `registry_for_agent` pattern is identical to xai-runner — clone the registry
before the heartbeat loop takes ownership of the original.

---

## Step 17 — agent.rs: egress policy for fetch_url

`fetch_url` without URL restrictions is a SSRF/exfiltration risk — a model
manipulated via prompt injection could read cloud metadata endpoints
(e.g. `http://169.254.169.254/latest/meta-data/`) and return credentials.

`EgressPolicy::default_safe()` blocks all link-local addresses (`169.254.x.x`)
while allowing everything else. Same pattern as xai-runner (lines 1313–1324).

### In the tool dispatch loop (Step 7)

Add a pre-check before dispatching `fetch_url`:

```rust
let result = if call.name == "bash" {
    // ... bash dispatch (Step 15)
} else if call.name == "fetch_url" {
    let input = serde_json::from_str::<serde_json::Value>(&call.arguments)
        .unwrap_or(Value::Null);
    let url = input.get("url").and_then(|v| v.as_str()).unwrap_or("");
    if !trogon_runner_tools::egress::EgressPolicy::default_safe().is_allowed(url) {
        format!("fetch_url: URL blocked by egress policy: {url}")
    } else {
        trogon_tools::dispatch_tool(&ctx, &call.name, &input).await
    }
} else {
    let input = serde_json::from_str::<serde_json::Value>(&call.arguments)
        .unwrap_or(Value::Null);
    trogon_tools::dispatch_tool(&ctx, &call.name, &input).await
};
```

No new fields on `OpenRouterAgent` — `EgressPolicy::default_safe()` is stateless.

---

## What is NOT in v1

| Feature | Reason deferred |
|---------|----------------|
| Permission gates | Requires `PermissionChecker` infrastructure — separate PR |
| MCP tools | Not needed for core coding use case |

---

## Post-implementation fixes

### Fix 1 — `enabled_tools` not persisted to KV

`build_snapshot` always stored `tools: vec![]` regardless of the session's enabled tools.
On runner restart, `load_session` could not restore the tool state.

**Changes:**
- `build_snapshot`: `tools: vec![]` → `tools: session.enabled_tools.clone()`
- `SessionStoring` trait: added `fn load`; `NatsSessionStore` implements it via SESSIONS KV
- `load_session`: falls back to KV snapshot when session is not in memory; restores
  `enabled_tools` from `snap.tools` (empty list on old snapshots defaults to all tools enabled
  for backward compatibility)

**Known limitation:** if all tools are disabled, `snap.tools` is `[]` — identical to the
old format — so the fallback re-enables all tools on restart. Disabling all tools is not
a realistic use case and resolving the ambiguity would require a `snapshot_version` field.

### Fix 2 — Stale JetStream messages on restart

`commands_observer` used `DeliverPolicy::All`, causing the runner to reprocess commands
published before the consumer was created on every restart.

**Change:** `commands_observer` in `acp-nats` — `DeliverPolicy::All` → `DeliverPolicy::New`.

**Trade-off:** a command in-flight when the runner crashes is not retried after restart.
Acceptable because the ACP client handles timeouts at the protocol level.
