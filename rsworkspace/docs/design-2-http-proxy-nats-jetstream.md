# Design 2 — HTTP Proxy + NATS JetStream Hybrid for AI Provider Token Exchange

## Context

The goal is to build a VGS-like system where services never hold real API keys for OpenAI/Anthropic/etc. Instead they hold tokens (`tok_anthropic_prod_abc`). An HTTP proxy intercepts outbound calls, exchanges the token for the real key via a vault, and forwards the request to the AI provider. NATS JetStream (hybrid) is used internally: the request is persisted in a JetStream stream (durable, survives worker crash), and the reply comes back via Core NATS (fast, synchronous).

## Architecture

```
Service → POST http://proxy:8080/anthropic/v1/messages
          Authorization: Bearer tok_anthropic_prod_abc
              ↓
         [HTTP Proxy (axum)]
              ↓ publishes OutboundHttpRequest to JetStream stream "PROXY_REQUESTS"
              ↓ subscribes to reply subject (Core NATS): trogon.proxy.reply.{uuid}
              ↓ waits up to 60s
         [Detokenization Worker (JetStream pull consumer)]
              ↓ resolves tok_anthropic_prod_abc → sk-ant-realkey via VaultStore
              ↓ calls https://api.anthropic.com/v1/messages with real key
              ↓ publishes response to trogon.proxy.reply.{uuid} (Core NATS)
         [HTTP Proxy receives reply, returns HTTP response to service]
```

## New Crates

### 1. `rsworkspace/crates/trogon-vault/`

Token storage with trait-based backends (follows existing `trogon-std` patterns).

**Files:**
- `Cargo.toml` — deps: serde, uuid, trogon-std
- `src/lib.rs` — pub use token::*, vault::*, backends::memory::*
- `src/token.rs` — `ApiKeyToken` (validated value object like `AcpPrefix`), `AiProvider` enum, `Env` enum, `TokenError`
- `src/vault.rs` — `VaultStore` trait: `store`, `resolve`, `revoke`
- `src/backends/mod.rs` — pub use memory
- `src/backends/memory.rs` — `MemoryVault` using `Arc<Mutex<HashMap>>` (Send+Sync, for tests)

**Token format:** `tok_{provider}_{env}_{id}` e.g. `tok_anthropic_prod_a1b2c3`

**VaultStore trait:**
```rust
pub trait VaultStore: Send + Sync {
    type Error: std::error::Error + Send + Sync;
    async fn store(&self, token: &ApiKeyToken, plaintext: &str) -> Result<(), Self::Error>;
    async fn resolve(&self, token: &ApiKeyToken) -> Result<Option<String>, Self::Error>;
    async fn revoke(&self, token: &ApiKeyToken) -> Result<(), Self::Error>;
}
```

### 2. `rsworkspace/crates/trogon-secret-proxy/`

HTTP proxy + NATS JetStream worker.

**Files:**
- `Cargo.toml` — deps: trogon-nats, trogon-vault, async-nats, axum, reqwest, tokio(full), serde, serde_json, tracing, bytes, uuid, futures-util
- `src/lib.rs` — pub modules
- `src/config.rs` — `Config` with prefix, nats config, proxy port (default 8080), worker timeout (default 60s)
- `src/messages.rs` — `OutboundHttpRequest`, `OutboundHttpResponse` (Serialize/Deserialize)
- `src/subjects.rs` — subject helpers: `outbound(prefix)`, `reply(prefix, id)`
- `src/stream.rs` — `ensure_stream()`: creates JetStream stream "PROXY_REQUESTS" if not exists
- `src/proxy.rs` — axum HTTP server, path-based routing `/{provider}/*path`, publishes to JetStream, waits on Core NATS reply
- `src/worker.rs` — JetStream pull consumer, vault lookup, reqwest call, Core NATS reply
- `src/provider.rs` — `AiProvider → base URL` mapping (openai→api.openai.com, anthropic→api.anthropic.com, etc.)

## Key Implementation Details

### HTTP Proxy routing (proxy.rs)
- Route: `/{provider}/*path` → strips provider prefix, builds full URL
- `GET/POST/PUT/DELETE /anthropic/v1/messages` → `https://api.anthropic.com/v1/messages`
- Generates `correlation_id = uuid::Uuid::new_v4()`
- Subscribes to `trogon.proxy.reply.{correlation_id}` (Core NATS, temporary) **before** publishing
- Publishes `OutboundHttpRequest` to JetStream subject `trogon.proxy.http.outbound`
- Waits on `reply_sub.next()` with `worker_timeout` (default 60s)

### JetStream Stream config (stream.rs)
```rust
StreamConfig {
    name: "PROXY_REQUESTS",
    subjects: vec!["trogon.proxy.http.outbound"],
    retention: RetentionPolicy::WorkQueue,  // deleted after ack
    storage: StorageType::Memory,           // fast
    ..Default::default()
}
```

> **Note:** `max_deliver` belongs on the consumer config (`pull::Config`), not the stream config.

### Worker detokenization (worker.rs)
- Pull consumer with `pull::Config { durable_name, ack_policy: Explicit, max_deliver: 3 }`
- Case-insensitive lookup for `Authorization` header
- Regex-equivalent: `^Bearer (tok_[a-zA-Z0-9_]+)$` on Authorization header
- `vault.resolve(token)` → real key
- Replace Authorization header, make `reqwest` call
- Publish `OutboundHttpResponse` to reply subject via Core NATS
- `msg.ack()` after successful reply; `msg.ack_with(AckKind::Nak(None))` on deserialization error

### Messages (messages.rs)
```rust
pub struct OutboundHttpRequest {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub reply_to: String,           // Core NATS reply subject
    pub idempotency_key: String,    // passed to AI provider as X-Request-Id
}

pub struct OutboundHttpResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub error: Option<String>,
}
```

## Patterns Reused from Existing Codebase

- `AcpPrefix` validation pattern → `ApiKeyToken` validated value object (`src/token.rs`)
- `MemFs` with `Arc<Mutex<>>` → `MemoryVault` (`src/backends/memory.rs`)
- `NatsError` enum pattern → `ProxyError`, `WorkerError` enums
- `Config::new().with_x()` builder pattern → `Config` in `trogon-secret-proxy`
- Edition 2024 async traits with `impl Future` (no `async_trait` macro)

## async-nats 0.45 API Notes

- Pull consumer requires `pull::Config` (not generic `consumer::Config`) for `.messages()` to compile
- `StreamConfig` has no `max_deliver` — that field lives on `pull::Config`
- `get_or_create_stream` returns `Result<Stream, CreateStreamError>` (not `Box<dyn Error>`)
- `AckKind` import path: `async_nats::jetstream::AckKind`
- `Subscriber::next()` requires `futures_util::StreamExt` in scope (add `futures-util` as dep)

## Files Created

```
rsworkspace/crates/trogon-vault/
  Cargo.toml
  src/lib.rs
  src/token.rs
  src/vault.rs
  src/backends/mod.rs
  src/backends/memory.rs

rsworkspace/crates/trogon-secret-proxy/
  Cargo.toml
  src/lib.rs
  src/config.rs
  src/messages.rs
  src/subjects.rs
  src/stream.rs
  src/proxy.rs
  src/worker.rs
  src/provider.rs
```

Workspace `Cargo.toml` requires no changes — `members = ["crates/*"]` already picks up new crates.

## Verification

```bash
cargo build -p trogon-vault          # ✅ compiles cleanly
cargo build -p trogon-secret-proxy   # ✅ compiles cleanly (no warnings)
cargo test -p trogon-vault           # ✅ 21 tests pass
cargo test -p trogon-secret-proxy    # ✅ 16 tests pass
```

## Manual End-to-End Test

```bash
# 1. Start NATS with JetStream enabled
nats-server -js

# 2. Start the worker (in your binary/main.rs)
#    Connect to NATS, call ensure_stream(), then worker::run(...)

# 3. Start the proxy
#    Connect to NATS, call ensure_stream(), then start axum server on :8080

# 4. Seed a token in the vault
#    vault.store(&ApiKeyToken::new("tok_anthropic_prod_abc123").unwrap(), "sk-ant-real...").await?;

# 5. Send a proxied request
curl -X POST http://localhost:8080/anthropic/v1/messages \
  -H "Authorization: Bearer tok_anthropic_prod_abc123" \
  -H "Content-Type: application/json" \
  -d '{"model":"claude-3-5-sonnet-20241022","max_tokens":10,"messages":[{"role":"user","content":"Hi"}]}'

# Expected: response from Anthropic using the real key, service never saw sk-ant-real...
```
