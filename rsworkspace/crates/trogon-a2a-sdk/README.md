# trogon-a2a-sdk

Identity-aware agent-to-agent SDK for TrogonStack. Application code configures **who it is** and **what it wants to do**; the SDK owns registry lookup, STS token exchange, mesh-token attachment, inbound verification, and OpenTelemetry spans.

Contract: [`docs/identity/sdk.md`](../../docs/identity/sdk.md).

## Quick start

Run the echo example against a local NATS broker with mesh JWT verification configured:

```bash
export NATS_URL=nats://127.0.0.1:4222
export AGENT_ID=acme/echo-agent
export MESH_JWT_HS256_SECRET=dev-only-change-me   # dev HS256 only; production uses STS RS256 + JWKS
cargo run -p trogon-a2a-sdk --example echo_agent --features nats
```

The example subscribes on `mcp.agent.{tenant}.{name}.request`, verifies inbound mesh JWTs, and echoes the payload.

### Client

```rust
use trogon_a2a_sdk::{AgentId, Client, Purpose};
use trogon_a2a_sdk::registry::NatsRegistry;
use trogon_a2a_sdk::sts::NatsSts;
use trogon_a2a_sdk::svid::FileSvidSource;

let nats = async_nats::connect("nats://127.0.0.1:4222").await?;
let client = Client::builder()
    .agent_id(AgentId::parse("acme/oncall-agent")?)
    .svid_source(FileSvidSource::new("/var/run/svid.jwt"))
    .subject_token_source(/* bootstrap JWT refresh */)
    .sts(NatsSts::new(nats.clone()))
    .registry(NatsRegistry::new(nats.clone()))
    .transport(nats.clone())
    .build()?;

// Pass an explicit purpose, or `None` to apply registry default-purpose rules (see sdk.md).
let reply: MyResponse = client
    .call(&AgentId::parse("acme/planner")?, &my_request, Some(&Purpose::new("handoff")))
    .await?;
```

### Server

```rust
use trogon_a2a_sdk::{AgentId, Handler, Hs256Jwks, serve, Caller};

struct MyHandler;

#[async_trait::async_trait]
impl Handler for MyHandler {
    async fn handle(&self, caller: Caller, raw_payload: bytes::Bytes) -> Result<bytes::Bytes, trogon_a2a_sdk::SdkError> {
        // Use caller.originator, caller.chain, caller.purpose — never parse JWTs by hand.
        Ok(raw_payload)
    }
}

serve(nats, AgentId::parse("acme/echo-agent")?, jwks, MyHandler).await?;
```

## Recipe: register and call a new agent

1. **Register both agents** in the agent registry (`mcp-agent-registry` KV) with `allowed_workloads`, `allowed_audiences` (include the callee `urn:trogon:a2a:agent:{tenant}:{name}`), and `allowed_purposes`.
2. **Run `trogon-sts`** so `Client::call` can exchange bootstrap tokens on `mcp.sts.exchange`.
3. **Build the callee** with `serve(nats, own_agent_id, jwks, handler)` so it queue-subscribes on `mcp.agent.{tenant}.{name}.request`.
4. **Build the caller** with `Client::builder()` wired to the same NATS connection for `NatsSts`, `NatsRegistry`, and `transport`.
5. **Call** with an explicit `Purpose` when the caller has multiple `allowed_purposes`; omit `purpose` only when the registry lists zero or one allowed purpose (see [Default purpose handling](../../docs/identity/sdk.md#default-purpose-handling)).

## Recipe: verify `act_chain` in a handler

After `serve()` verifies the inbound mesh token, the handler receives a typed `Caller`:

```rust
async fn handle(&self, caller: Caller, payload: Bytes) -> Result<Bytes, SdkError> {
    assert_eq!(caller.chain_depth(), caller.chain.len());
    assert_eq!(caller.originator.sub, caller.chain.first().map(|e| e.sub.as_str()).unwrap_or(""));
    assert_eq!(caller.direct.sub, caller.chain.last().map(|e| e.sub.as_str()).unwrap_or(""));
    if caller.chain_depth() > 4 {
        return Err(SdkError::Forbidden("chain too long for this handler".into()));
    }
    Ok(payload)
}
```

Do not decode JWTs or read `A2a-Caller-Jwt` in application code.

## Wiring real STS and registry

| Component | Trait | NATS impl (feature `nats`, default) |
|-----------|-------|-------------------------------------|
| Workload attestation | `SvidSource` | `FileSvidSource` (dev) or SPIRE adapter (prod) |
| Token exchange | `Sts` | `NatsSts` → `mcp.sts.exchange` |
| Agent lookup | `Registry` | `NatsRegistry` → `mcp.registry.agent.lookup` |
| Mesh JWT verify | `Jwks` | `Rs256Jwks` / JWKS from STS KV (ADR 0006); `Hs256Jwks` for local dev only |

Pass the same `async_nats::Client` to `NatsSts`, `NatsRegistry`, and `Client::transport`. The SDK never mints tokens; it only calls STS and validates inbound mesh JWTs.

For unit tests and offline development, implement the traits with mocks — no live NATS required.

## CI lint: direct gateway publishes

Agent application crates must not publish to `mcp.gateway.request.*` directly. CI runs:

```bash
bash scripts/ci/check-direct-nats-publishes.sh
```

Only `trogon-a2a-sdk` (and test paths) may reference that subject alongside `.publish(`.

## Live integration tests

End-to-end tests boot NATS (`nats-server` on `PATH`), in-process registry + STS consumers, and exercise `Client::call` against `serve`:

```bash
cargo test -p trogon-a2a-sdk --features live integration_live
```

Without `--features live`, the integration test is ignored.

## Features

- `nats` (default): NATS-backed `NatsSts` and `NatsRegistry`.
- `live`: enables `tests/integration_live.rs` (requires `nats-server` on `PATH`).

Disable defaults with `default-features = false` when using mock implementations only.
