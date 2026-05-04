# Streaming Responses — Implementation Plan

## Current State

The full pipeline buffers every AI provider response before returning anything:

```
Agent → Proxy (Axum) → JetStream → Worker → AI Provider
                                              ↓ (full response buffered)
Agent ← Proxy (Axum) ← Core NATS  ← Worker
```

Key bottlenecks:

- `ReqwestAnthropicClient::complete()` calls `.json::<serde_json::Value>().await` — waits for the entire response body
- `HttpClient::send_request()` in the worker calls `.bytes().await` — same problem
- `OutboundHttpResponse.body: Vec<u8>` carries the full body in a single NATS message
- The proxy blocks on `reply_sub.next()` waiting for one message that contains everything

`run_chat_streaming` already exists in `trogon-agent-core` with `AgentEvent` variants (`TextDelta`, `ToolCallStarted`, etc.) but currently calls the same non-streaming `complete()` — so events fire after the full response arrives, not as chunks come in.

The client-facing endpoint (`POST /sessions/:id/messages`) does not change in this plan.

---

## Architecture After This Plan

```
Agent → Proxy (Axum) → JetStream → Worker → AI Provider (stream: true)
                                              ↓ SSE chunks
Agent ← Proxy (Axum) ← Core NATS  ← Worker  (N messages: start, chunks, end)
         chunked HTTP
```

The worker detects `"stream": true` in the request body, calls the AI provider with streaming enabled, and publishes each chunk as a separate NATS message to the reply subject. The proxy reassembles them into a chunked HTTP response back to the agent. The agent parses SSE events and emits `AgentEvent::TextDelta` as each chunk arrives.

---

## Phase 1 — NATS Streaming Message Protocol

**File:** `crates/trogon-secret-proxy/src/messages.rs`

Add three new message types to replace the single `OutboundHttpResponse` for streaming responses. The reply subject stays the same — the worker sends multiple messages to it, distinguished by a NATS header `X-Stream-Frame: start | chunk | end`.

```rust
/// First message on the reply subject for streaming responses.
/// Contains status and headers; body arrives in subsequent chunks.
pub struct OutboundStreamStart {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

/// One body chunk. Sent N times until all data is delivered.
pub struct OutboundStreamChunk {
    pub seq: u64,
    pub data: Vec<u8>,
}

/// Final message. Signals end of stream or an error mid-stream.
pub struct OutboundStreamEnd {
    pub error: Option<String>,
}
```

Header values: `X-Stream-Frame: start`, `X-Stream-Frame: chunk`, `X-Stream-Frame: end`.

Non-streaming responses continue to use `OutboundHttpResponse` exactly as today — the worker decides which protocol to use based on whether `stream: true` appears in the request body JSON.

---

## Phase 2 — Streaming HTTP Client Trait

**File:** `crates/trogon-secret-proxy/src/traits.rs`

Add a streaming variant to `HttpClient`. The existing `send_request` stays unchanged.

```rust
pub trait HttpClient: Clone + Send + Sync + 'static {
    fn send_request(...) -> impl Future<Output = Result<HttpResponse, String>> + Send;

    fn send_request_streaming(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> impl Future<Output = Result<StreamingHttpResponse, String>> + Send;
}

pub struct StreamingHttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub chunks: Pin<Box<dyn Stream<Item = Result<Bytes, String>> + Send>>,
}
```

**File:** `crates/trogon-secret-proxy/src/impls.rs`

Implement `send_request_streaming` on `reqwest::Client`:

```rust
async fn send_request_streaming(...) -> Result<StreamingHttpResponse, String> {
    let resp = builder.send().await.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let headers = /* extract headers */;
    let chunks = resp.bytes_stream().map(|r| r.map_err(|e| e.to_string()));
    Ok(StreamingHttpResponse { status, headers, chunks: Box::pin(chunks) })
}
```

---

## Phase 3 — Worker Streaming Path

**File:** `crates/trogon-secret-proxy/src/worker.rs`

In `process_request`, detect whether the request body contains `"stream": true`:

```rust
let is_streaming = serde_json::from_slice::<serde_json::Value>(&request.body)
    .ok()
    .and_then(|v| v.get("stream").and_then(|s| s.as_bool()))
    .unwrap_or(false);
```

If `true`, call `http_client.send_request_streaming()` instead of `send_request()`, then publish frames to `request.reply_to`:

```rust
// 1. Publish start frame
nats.publish_with_header(reply_to, "X-Stream-Frame", "start",
    serde_json::to_vec(&OutboundStreamStart { status, headers })?).await;

// 2. Publish each chunk
let mut seq = 0u64;
while let Some(chunk) = streaming_resp.chunks.next().await {
    nats.publish_with_header(reply_to, "X-Stream-Frame", "chunk",
        serde_json::to_vec(&OutboundStreamChunk { seq, data: chunk? })?).await;
    seq += 1;
}

// 3. Publish end frame
nats.publish_with_header(reply_to, "X-Stream-Frame", "end",
    serde_json::to_vec(&OutboundStreamEnd { error: None })?).await;
```

The fallback-on-401 logic applies only to the non-streaming path (a 401 during streaming means the stream has already started, so we cannot retry transparently).

The JetStream ack for the original request is sent after the `end` frame is published.

**File:** `crates/trogon-secret-proxy/src/bin/worker.rs`

No changes needed — it already wires the `HttpClient` and `NatsClient` into the library worker loop.

---

## Phase 4 — Proxy Streaming Assembly

**File:** `crates/trogon-secret-proxy/src/proxy.rs`

After publishing `OutboundHttpRequest` to JetStream, detect whether the original HTTP request body contains `"stream": true`. If so, enter streaming receive mode:

```rust
// Receive start frame
let start_msg = timeout(worker_timeout, reply_sub.next()).await??;
assert header X-Stream-Frame == "start"
let start: OutboundStreamStart = serde_json::from_slice(&start_msg.payload)?;

// Begin streaming HTTP response to the agent
let (tx, body) = axum::body::Body::channel();
// Send HTTP response headers immediately with chunked transfer encoding
spawn task:
    loop {
        let msg = timeout(chunk_timeout, reply_sub.next()).await??;
        match msg.header("X-Stream-Frame") {
            "chunk" => tx.send_data(chunk.data),
            "end"   => break,
        }
    }

return Response::builder().status(start.status).body(body)
```

The proxy no longer waits for the entire response before returning — it starts sending as soon as it receives the `start` frame and status code.

---

## Phase 5 — Streaming AnthropicClient in trogon-agent

**File:** `crates/trogon-agent/src/agent_loop.rs` and `crates/trogon-agent-core/src/agent_loop.rs`

Add a streaming method to the `AnthropicClient` trait:

```rust
pub trait AnthropicClient: Send + Sync + 'static {
    fn complete<'a>(&'a self, body: serde_json::Value)
        -> Pin<Box<dyn Future<Output = Result<serde_json::Value, reqwest::Error>> + Send + 'a>>;

    fn complete_streaming<'a>(&'a self, body: serde_json::Value)
        -> Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'a>>;
}
```

Implement `complete_streaming` on `ReqwestAnthropicClient`:

```rust
fn complete_streaming<'a>(&'a self, mut body: serde_json::Value)
    -> Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'a>>
{
    body["stream"] = json!(true);
    Box::pin(async_stream::stream! {
        let resp = self.http
            .post(format!("{}/anthropic/v1/messages", self.proxy_url))
            .header("Authorization", format!("Bearer {}", self.anthropic_token))
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await?;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            yield chunk;
        }
    })
}
```

---

## Phase 6 — Wire run_chat_streaming to the Streaming Client

**File:** `crates/trogon-agent-core/src/agent_loop.rs`

`run_chat_streaming` currently calls `anthropic_client.complete()` and emits events after receiving the full response. Change it to call `complete_streaming()` and parse the SSE response incrementally:

```rust
// Build request body with stream: true
let mut body = build_request_body(&messages, tools, system_prompt, &self.model);
body["stream"] = json!(true);

let mut stream = self.anthropic_client.complete_streaming(body);
let mut text_buffer = String::new();
let mut sse_parser = SseParser::new();

while let Some(chunk) = stream.next().await {
    let bytes = chunk.map_err(AgentError::Http)?;
    for event in sse_parser.feed(&bytes) {
        match event {
            SseEvent { event: "content_block_delta", data } => {
                if let Some(delta) = data["delta"]["text"].as_str() {
                    text_buffer.push_str(delta);
                    event_tx.send(AgentEvent::TextDelta { text: delta.to_string() }).await.ok();
                }
            }
            SseEvent { event: "message_delta", data } => {
                // extract usage, stop_reason
            }
            // ... other event types
        }
    }
}
```

This is where `TextDelta` events start firing as the LLM produces tokens, not after the full response arrives.

---

## What Does Not Change

- `POST /sessions/:id/messages` — still returns a single JSON response, unchanged
- `run_chat()` — still exists and works the same way for callers that don't need streaming
- The JetStream work-queue that distributes requests from proxy to workers — unchanged
- The vault/token resolution in the worker — unchanged
- All existing tests — should pass without modification

---

## Implementation Order

| Phase | What | Effort |
|---|---|---|
| 1 | NATS streaming message types | 0.5 day |
| 2 | Streaming HttpClient trait + reqwest impl | 0.5 day |
| 3 | Worker streaming path | 1 day |
| 4 | Proxy streaming assembly | 1 day |
| 5 | Streaming AnthropicClient trait + impl | 0.5 day |
| 6 | Wire run_chat_streaming to streaming client | 1 day |

Total estimated: **4–4.5 days**

Phases 1–2 are pure additions (no breaking changes). Phases 3–4 extend existing logic behind an `is_streaming` branch. Phases 5–6 extend the trait with a new method — existing `complete()` callers are unaffected.

---

## Testing Strategy

- **Unit tests (phases 1–2):** Test message serialization/deserialization; mock streaming HTTP client that emits N chunks followed by end.
- **Integration tests (phases 3–4):** Use testcontainers NATS; send a request with `stream: true` to the proxy, assert chunks arrive in order and the full body reassembles correctly.
- **Integration tests (phases 5–6):** Mock a streaming Anthropic endpoint (returns SSE events); assert `AgentEvent::TextDelta` events are emitted incrementally, not all at once after the response completes.
