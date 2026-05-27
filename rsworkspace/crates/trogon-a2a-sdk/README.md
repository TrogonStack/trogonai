# trogon-a2a-sdk

Identity-aware agent-to-agent SDK for TrogonStack. Application code configures **who it is** and **what it wants to do**; the SDK owns registry lookup, STS token exchange, mesh-token attachment, inbound verification, and OpenTelemetry spans.

Contract: [`docs/identity/sdk.md`](../../docs/identity/sdk.md).

## Quick start (client)

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

let reply: MyResponse = client
    .call(&AgentId::parse("acme/planner")?, &my_request, Some(&Purpose::new("handoff")))
    .await?;
```

## Quick start (server)

```rust
use trogon_a2a_sdk::{AgentId, Handler, Hs256Jwks, serve};

// Implement Handler — receive typed Caller, return Bytes
serve(nats, AgentId::parse("acme/echo-agent")?, jwks, MyHandler).await?;
```

See [`examples/echo_agent.rs`](examples/echo_agent.rs).

## Wiring real STS and registry

Once [`trogon-sts`](../../PENDING_TODO.md) and the agent registry (Block 1.1) land:

| Component | Trait | NATS impl (feature `nats`, default) |
|-----------|-------|-------------------------------------|
| Workload attestation | `SvidSource` | `FileSvidSource` (dev) or SPIRE adapter (prod) |
| Token exchange | `Sts` | `NatsSts` → `mcp.sts.exchange` |
| Agent lookup | `Registry` | `NatsRegistry` → `mcp.registry.agent.lookup` |
| Mesh JWT verify | `Jwks` | JWKS from STS KV / HTTP mirror (ADR 0006) |

Pass the same `async_nats::Client` to `NatsSts`, `NatsRegistry`, and `Client::transport`. The SDK never mints tokens; it only calls STS and validates inbound mesh JWTs.

For unit tests and offline development, implement the traits with mocks — no live NATS required.

## Features

- `nats` (default): NATS-backed `NatsSts` and `NatsRegistry`.

Disable with `default-features = false` when using mock implementations only.
