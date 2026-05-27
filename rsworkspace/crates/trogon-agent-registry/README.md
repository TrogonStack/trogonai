# trogon-agent-registry

Runtime Agent Registry service: NATS KV bucket `mcp-agent-registry`, request/reply lookup on `mcp.registry.agent.lookup`, and audit hooks on `mcp.audit.registry.*`.

Schema and lookup contract: `docs/identity/registry.md`.

## Run

```bash
export NATS_URL=nats://127.0.0.1:4222
export TROGON_REGISTRY_AUTOCREATE=1   # dev only; production buckets are operator-provisioned

cargo run -p trogon-agent-registry --bin trogon-agent-registry
```

## Tests

Unit tests run by default:

```bash
cargo test -p trogon-agent-registry
```

Integration tests require a reachable NATS server with JetStream enabled:

```bash
cargo test -p trogon-agent-registry -- --ignored
```

## Environment

| Variable | Description |
|---|---|
| `NATS_URL` | NATS server URL (default `nats://127.0.0.1:4222`) |
| `TROGON_REGISTRY_AUTOCREATE` | Set to `1` to create the KV bucket when missing (dev) |
| `RUST_LOG` | Tracing filter (default `info`) |
