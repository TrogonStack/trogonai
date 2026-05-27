# trogon-agent-registry-controller

Git-as-source-of-truth control plane for the Agent Registry. Watches a signed manifest repository, validates Ed25519 signatures, and projects records into NATS KV bucket `mcp-agent-registry`. Only this service (via its dedicated NATS account) should hold KV write ACLs; runtime lookup nodes remain read-only.

Companion runtime service: `trogon-agent-registry` (lookup on `mcp.registry.agent.lookup`).

## Configuration

| Variable | Description |
|---|---|
| `NATS_URL` | NATS server URL (default `nats://127.0.0.1:4222`) |
| `TROGON_REGISTRY_AUTOCREATE` | Set to `1` to create the KV bucket when missing (dev only) |
| `TROGON_REGISTRY_CONTROLLER_REPO_PATH` | Local clone of the manifest Git repository |
| `TROGON_REGISTRY_CONTROLLER_GIT_REMOTE` | Optional remote name/URL; when set, controller runs `git fetch` + `git pull --ff-only` before each sync |
| `TROGON_REGISTRY_CONTROLLER_GIT_REF` | Branch or ref to sync (default `main`) |
| `TROGON_REGISTRY_CONTROLLER_AGENTS_DIR` | Manifest root inside the repo (default `agents/`) |
| `TROGON_REGISTRY_CONTROLLER_SYNC_INTERVAL_S` | Poll interval in seconds (default `30`) |
| `TROGON_REGISTRY_CONTROLLER_SIGNER_SOURCE` | `file` (dev) or `kms` (prod stub) |
| `TROGON_REGISTRY_CONTROLLER_SIGNER_KEY_PATH` | Ed25519 PKCS#8 PEM path when `SIGNER_SOURCE=file` |

Production KMS signing is a stub (`todo!("kms backend selection — tracked in carry-over §JWKS production custody")`); wire a `ManifestSigner` backend when JWKS custody lands.

## Startup sequence

1. Connect to NATS with the controller service account (must have KV `PUT`/`DELETE` on `mcp-agent-registry`).
2. Open (or auto-create in dev) KV bucket `mcp-agent-registry`.
3. Load the Ed25519 signer (`file` PEM or future KMS backend).
4. Loop: pull Git → discover `agents/**/*.toml` → verify signature + schema → write versioned keys + `@latest` pointer → emit audit events → sleep until next interval or shutdown signal.

## Audit events

Controller mutations emit lifecycle-specific subjects (distinct from runtime lookup audit hooks on `mcp.audit.registry.lookup.*` and legacy `put`/`delete` subjects on the lookup service):

| Subject | When |
|---|---|
| `mcp.audit.registry.registered` | First manifest for an `agent_id` |
| `mcp.audit.registry.bumped` | New `agent_version` for an existing `agent_id` |
| `mcp.audit.registry.deprecated` | `lifecycle_state` set to `deprecated` |
| `mcp.audit.registry.revoked` | `lifecycle_state` set to `revoked` |

Payload includes `agent_id`, `agent_version`, lifecycle transition, `manifest_digest`, `git_commit`, `operator`, and `kv_revision`.

## Local validation before commit

Use `agctl registry sync` in CI or pre-commit hooks:

```bash
agctl registry sync \
  --repo /path/to/manifests \
  --verify-key /etc/trogon/registry-signer.pub.pem
```

Pass the dev examples key for smoke tests:

```bash
agctl registry sync \
  --repo rsworkspace/crates/trogon-agent-registry/examples \
  --agents-dir . \
  --verify-key rsworkspace/crates/trogon-agent-registry/examples/dev-signer.pem
```

## Emergency revocation

1. Edit the agent manifest in Git: set `lifecycle_state = "revoked"` (optionally clear `allowed_workloads`).
2. Merge to `main`; controller sync applies within one interval.
3. Confirm audit event on `mcp.audit.registry.revoked`.
4. If Git is unavailable, break-glass: direct KV put with operator dual-control (documented in `docs/identity/registry-operations.md`), then revert Git to match.

## Run

```bash
export NATS_URL=nats://127.0.0.1:4222
export TROGON_REGISTRY_AUTOCREATE=1
export TROGON_REGISTRY_CONTROLLER_REPO_PATH=/path/to/agent-manifests
export TROGON_REGISTRY_CONTROLLER_SIGNER_KEY_PATH=/etc/trogon/registry-signer.pem

cargo run -p trogon-agent-registry-controller
```

## Tests

```bash
cargo test -p trogon-agent-registry-controller
cargo clippy -p trogon-agent-registry-controller --all-targets -- -D warnings
```

Regenerate example manifests after schema changes:

```bash
cargo test -p trogon-agent-registry-controller export_example_manifests -- --ignored --nocapture
```
