# trogon-agent-registry

Runtime Agent Registry service: NATS KV bucket `mcp-agent-registry`, request/reply lookup on `mcp.registry.agent.lookup`, and audit hooks on `mcp.audit.registry.*`.

Schema and lookup contract: `docs/identity/registry.md`.

Control plane (Git sync + signed manifests): `trogon-agent-registry-controller`. Operator procedures: `docs/identity/registry-operations.md`.

## Run

```bash
export NATS_URL=nats://127.0.0.1:4222
export TROGON_REGISTRY_AUTOCREATE=1   # dev only; production buckets are operator-provisioned

cargo run -p trogon-agent-registry --bin trogon-agent-registry
```

## Control plane (operator notes)

- **Source of truth:** signed TOML manifests in Git (`agents/**/*.toml`). See `docs/identity/registry.md` § Signed manifest schema.
- **Sync service:** `trogon-agent-registry-controller` is the only writer to KV `mcp-agent-registry`.
- **Pre-commit validation:** `agctl registry sync --repo <path> --verify-key <pem>`.
- **Emergency revocation:** set `lifecycle_state = "revoked"` in Git (preferred) or break-glass KV put — full steps in `docs/identity/registry-operations.md`.
- **Example manifests:** `examples/*.toml` signed with `examples/dev-signer.pem` (dev/CI only).

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
