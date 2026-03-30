# trogon-xai-runner

ACP agent that exposes xAI's Grok models as a service over NATS.

## What it does

`trogon-xai-runner` listens on NATS and implements the [Agent Client Protocol (ACP)](https://agentclientprotocol.com). Clients send prompts over NATS; the runner forwards them to xAI's Grok API, streams the response back chunk by chunk, and persists session state in NATS KV.

```
ACP client → NATS → trogon-xai-runner → api.x.ai (Grok) → streaming SSE → NATS → ACP client
```

## Architecture

### REST vs gRPC

xAI exposes two API surfaces:
- **OpenAI-compatible REST** at `api.x.ai/v1/chat/completions`
- **Native gRPC** defined in [`xai-org/xai-proto`](https://github.com/xai-org/xai-proto)

This crate uses the REST endpoint. See the decision record in [`src/client.rs`](src/client.rs) for the full rationale. In short: REST has feature parity for chat, there is no official Rust gRPC SDK for xAI, and the OpenAI-compatible endpoint keeps the client simple and portable.

### Components

| File | Responsibility |
|------|---------------|
| `src/client.rs` | HTTP client for xAI REST API. Stateful SSE parser that accumulates tool call deltas and emits `XaiEvent`s. |
| `src/session_store.rs` | `SessionStore` trait + NATS KV implementation (`KvSessionStore`) + in-memory implementation for tests (`MemorySessionStore`). |
| `src/agent.rs` | `XaiAgent` — implements the ACP `Agent` trait. Manages sessions, dispatches prompts, streams responses. |
| `src/main.rs` | Binary entry point. Reads env vars, connects to NATS, starts the agent. |

### Session lifecycle

Sessions are persisted in a NATS KV bucket (`XAI_SESSIONS` by default) with a configurable TTL (default 7 days). Each session stores:
- `cwd` — working directory (metadata, not used for file operations)
- `model` — active Grok model (overrides server default)
- `history` — conversation history (user + assistant messages)
- `api_key` — per-session xAI API key (optional if server key is configured)
- `system_prompt` — injected as first message on every turn
- `search_mode` — xAI server-side web search (`off` / `auto` / `on`)

### Authentication

Two auth methods:

1. **`xai-api-key`** — client provides their own xAI API key via `authenticate` before creating a session.
2. **`agent`** — client uses the server-wide key configured via `XAI_API_KEY` env var.

### Web search

xAI's web search is server-side via `search_parameters.mode` in the Chat Completions API — no tool call round-trips required. Configured per session via `set_session_config_option` with `config_id: "search_mode"`.

### Streaming

Prompt responses are streamed as `SessionNotification` (`agent_message_chunk`) over NATS. The runner uses a per-chunk inactivity timeout (`XAI_PROMPT_TIMEOUT_SECS`, default 300s) — the timeout fires if no chunk arrives within the duration, not on total prompt time.

### History truncation

History is trimmed to `XAI_MAX_HISTORY_MESSAGES` (default 20). Trimming always removes complete user/assistant pairs (rounds up to even) to preserve conversation structure.

## Known limitations

### `pending_api_key` single-slot

The `authenticate` → `new_session` correlation uses a single in-memory slot. If two clients authenticate concurrently before calling `new_session`, the second key overwrites the first. Fixing this requires the framework (`acp-nats-agent`) to expose a connection identifier to the agent handler. Tracked as a FIXME in `src/agent.rs`.

### Tool call round-trip

When Grok requests a tool call, the runner notifies the ACP client (`ToolCallStatus::Pending`) but cannot complete the round-trip. The `Message` struct only carries `{role, content}` — it does not support the `tool_call_id` field xAI expects in `role: tool` responses. For web search, use `search_mode` instead — xAI handles it server-side without tool calls.

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
| `XAI_SYSTEM_PROMPT` | _(unset)_ | System prompt injected at the start of every turn |

## Running

```bash
XAI_API_KEY=xai-your-key \
NATS_URL=nats://localhost:4222 \
cargo run -p trogon-xai-runner
```

## Testing

Unit and integration tests use a fake HTTP server and fake NATS — no real credentials needed:

```bash
cargo test -p trogon-xai-runner --features test-helpers -- --test-threads=1
```

KV integration tests require a real NATS server with JetStream:

```bash
NATS_TEST_URL=nats://localhost:4222 \
cargo test -p trogon-xai-runner --features test-helpers -- --test-threads=1 --include-ignored
```

A real xAI SSE response fixture is available at `tests/fixtures/grok3_hello_response.sse` — captured from a live Grok-3 call to verify the SSE format matches what `client.rs` expects.
