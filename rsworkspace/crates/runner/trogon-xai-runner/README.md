# trogon-xai-runner

ACP agent that exposes xAI's Grok models as a service over NATS.

## What it does

`trogon-xai-runner` listens on NATS and implements the [Agent Client Protocol (ACP)](https://agentclientprotocol.com). Clients send prompts over NATS; the runner forwards them to xAI's Grok **Responses API**, streams the response back chunk by chunk, persists session state in NATS KV, and runs a multi-turn agentic loop for server-side tool calls.

```
ACP client → NATS → trogon-xai-runner → api.x.ai/v1/responses (Grok) → SSE → NATS → ACP client
```

## Architecture

### API surface — Responses API

xAI exposes three API surfaces:

| Surface | Endpoint | Used here |
|---------|----------|-----------|
| **Responses API** | `api.x.ai/v1/responses` | ✅ |
| OpenAI-compatible chat | `api.x.ai/v1/chat/completions` | ✗ |
| Native gRPC | `xai-org/xai-proto` (`chat.proto` full Chat service) | ✗ |

The Responses API is used because:

1. **Server-side tools** — `web_search`, `x_search`, `code_interpreter`, `file_search` are only available here. No client-side tool round-trips required; xAI executes the tools internally.
2. **Stateful multi-turn** — `previous_response_id` lets subsequent requests reference prior context without re-sending the full history.
3. **Agentic design** — the Responses API is purpose-built for agentic loops.

The gRPC surface (`xai-org/xai-proto`, `chat.proto`) is more capable than its name suggests: it supports streaming (`GetCompletionChunk`), `previous_response_id`, `max_turns`, `search_parameters`, `agent_count`, and `use_encrypted_content`. However, its tool execution model is fundamentally different: `Delta.tool_calls` in streaming chunks requests *client-side* function call round-trips — the client executes the function and returns results. The Responses API executes tools server-side and notifies the client via SSE events without any round-trip. For a runner where xAI handles tool execution transparently, the Responses API is the correct choice. The choice is also pragmatic: SSE streaming is already in production, there is no official Rust gRPC SDK, and gRPC would add `tonic`/`prost` codegen with no functional gain.

See the full decision record in [`src/client.rs`](src/client.rs).

### Components

| File | Responsibility |
|------|---------------|
| `src/client.rs` | HTTP client for xAI Responses API. Stateful SSE parser that emits `XaiEvent`s (text deltas, function calls, server-side tool completions, finish reasons). |
| `src/session_store.rs` | `SessionStore` trait + NATS KV implementation (`KvSessionStore`) + in-memory implementation for tests (`MemorySessionStore`). |
| `src/agent.rs` | `XaiAgent` — implements the ACP `Agent` trait. Manages sessions, dispatches prompts, runs the agentic loop. |
| `src/main.rs` | Binary entry point. Reads env vars, connects to NATS, starts the agent. |

### Session lifecycle

Sessions are persisted in a NATS KV bucket (`XAI_SESSIONS` by default) with a configurable TTL (default 7 days). Each session stores:

| Field | Description |
|-------|-------------|
| `cwd` | Working directory (metadata) |
| `model` | Active Grok model (overrides server default) |
| `history` | Conversation history (user + assistant messages) |
| `api_key` | Per-session xAI API key (optional if server key is configured) |
| `system_prompt` | Injected as first message of the conversation |
| `enabled_tools` | Server-side tools active for this session (`web_search`, `x_search`, etc.) |
| `last_response_id` | ID of the last xAI response; used as `previous_response_id` on the next turn to avoid re-sending full history |

### Authentication

Two auth methods:

1. **`xai-api-key`** — client provides their own xAI API key via `authenticate` before creating a session.
2. **`agent`** — client uses the server-wide key configured via `XAI_API_KEY` env var.

### Server-side tools

xAI's built-in tools are toggled per session via `set_session_config_option` using the tool ID as `config_id`:

| Tool | `config_id` |
|------|-------------|
| Web search | `web_search` |
| X/Twitter search | `x_search` |
| Code interpreter | `code_interpreter` |
| File search | `file_search` |

When enabled, xAI executes the tools internally — no client-side round-trip required. The runner tracks tool call lifecycle events in the SSE stream and forwards `ToolCall` status notifications to the ACP client:

- `function_call` event → `ToolCallStatus::Pending`
- `response.*_call.completed` → `ToolCallStatus::Completed` (emitted mid-stream, before model text generation starts)
- `[DONE]` or stream end → remaining tool calls resolved as Completed (or Failed on error)

### Agentic loop

The outer loop handles model responses that are cut short (`response.incomplete`):

- **`max_output_tokens`** — model was truncated mid-text; sends continuation with empty input so the model resumes exactly where it stopped.
- **`max_turns`** — tool-call iterations exhausted; re-sends the original user question so the model produces its final answer using tool results already in context.
- Up to `MAX_CONTINUATIONS = 5` iterations; returns `StopReason::Cancelled` with partial text if exceeded.

### Stateful multi-turn

When `last_response_id` is set, each prompt sends only the new user message with `previous_response_id`. On error (stale/expired ID), the agent retries once with full history (transparent recovery). The ID is cleared on model switch and fork.

### Streaming

Prompt responses are streamed as `SessionNotification` (`agent_message_chunk`) over NATS. A per-chunk inactivity timeout (`XAI_PROMPT_TIMEOUT_SECS`, default 300s) fires if no chunk arrives within the duration — not on total prompt time.

### History truncation

History is trimmed to `XAI_MAX_HISTORY_MESSAGES` (default 20). Trimming removes complete user/assistant pairs from the front; orphaned user messages (from crashed turns) are handled individually.

## Known limitations

### `pending_api_key` single-slot

The `authenticate` → `new_session` correlation uses a single in-memory slot. If two clients authenticate concurrently before calling `new_session`, the second key overwrites the first. Fixing this requires the framework (`acp-nats-agent`) to expose a connection identifier to the agent handler. Tracked as a FIXME in `src/agent.rs`.

### Custom function calls not supported

All tools are xAI server-side built-ins. The runner has no mechanism for client-side function call round-trips — the agent notifies the ACP client of tool calls via `ToolCallStatus` but never submits results back to xAI. Custom tools outside the four built-ins are not supported.

## Configuration

| Env var | Default | Description |
|---------|---------|-------------|
| `NATS_URL` | `nats://localhost:4222` | NATS server URL |
| `ACP_PREFIX` | `acp` | NATS subject prefix |
| `XAI_API_KEY` | _(empty)_ | Server-wide xAI API key |
| `XAI_BASE_URL` | `https://api.x.ai/v1` | xAI API base URL (override for testing) |
| `XAI_DEFAULT_MODEL` | `grok-3` | Default Grok model |
| `XAI_MODELS` | `grok-3:Grok 3,grok-3-mini:Grok 3 Mini` | Available models (`id:label` pairs, comma-separated) |
| `XAI_SESSION_BUCKET` | `XAI_SESSIONS` | NATS KV bucket name |
| `XAI_SESSION_TTL_SECS` | `604800` (7 days) | Session TTL in seconds (0 = no expiry) |
| `XAI_PROMPT_TIMEOUT_SECS` | `300` | Per-chunk inactivity timeout in seconds |
| `XAI_MAX_HISTORY_MESSAGES` | `20` | Maximum messages kept in history |
| `XAI_MAX_TURNS` | `10` | Maximum agentic tool-call iterations per prompt |
| `XAI_SYSTEM_PROMPT` | _(unset)_ | System prompt injected at the start of every conversation |

## Running

```bash
XAI_API_KEY=xai-your-key \
NATS_URL=nats://localhost:4222 \
cargo run -p trogon-xai-runner
```

## Testing

Unit and integration tests use a fake HTTP server and fake NATS — no real credentials needed:

```bash
cargo test -p trogon-xai-runner --features test-helpers
```

KV integration tests require a real NATS server with JetStream:

```bash
NATS_TEST_URL=nats://localhost:4222 \
cargo test -p trogon-xai-runner --features test-helpers -- --include-ignored
```
