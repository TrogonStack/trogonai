# A2A runtime environment reference

Environment variables and CLI flags for A2A binaries and embedders. Shared timeout and prefix knobs live in the **`a2a_nats`** crate (`rsworkspace/crates/a2a-nats`).

## Shared `a2a_nats` configuration

Embedders build an [`a2a_nats::Config`](../../rsworkspace/crates/a2a-nats/src/config.rs) (prefix + `trogon_nats::NatsConfig`), then apply runtime overrides:

| Function | Purpose |
|----------|---------|
| [`apply_timeout_overrides`](../../rsworkspace/crates/a2a-nats/src/config.rs) | Reads env and overrides operation timeout, task timeout, max concurrent client tasks, and push DLQ caller segment on an existing `Config`. |
| [`nats_connect_timeout`](../../rsworkspace/crates/a2a-nats/src/config.rs) | Returns NATS dial timeout from env (independent of `Config`). |

### Exported env constant names (`a2a_nats`)

Rust identifiers map to these **process environment variable names** (string values):

| Rust constant | Env var string | Default (when unset) |
|---------------|----------------|----------------------|
| `ENV_A2A_PREFIX` | `A2A_PREFIX` | `a2a` |
| `ENV_OPERATION_TIMEOUT_SECS` | `A2A_OPERATION_TIMEOUT_SECS` | 30 s (unary JSON-RPC) |
| `ENV_TASK_TIMEOUT_SECS` | `A2A_TASK_TIMEOUT_SECS` | 7200 s (streaming task completion) |
| `ENV_CONNECT_TIMEOUT_SECS` | `A2A_CONNECT_TIMEOUT_SECS` | 10 s |
| `ENV_PUSH_DLQ_CALLER_SEGMENT` | `A2A_PUSH_DLQ_CALLER_SEGMENT` | `_` |
| `ENV_MAX_CONCURRENT_CLIENT_TASKS` | `A2A_MAX_CONCURRENT_CLIENT_TASKS` | `256` (`0` normalizes to `1`) |

Constants are defined in [`constants.rs`](../../rsworkspace/crates/a2a-nats/src/constants.rs). The crate root re-exports `ENV_A2A_PREFIX`, `ENV_MAX_CONCURRENT_CLIENT_TASKS`, and `ENV_PUSH_DLQ_CALLER_SEGMENT`; the timeout env names are used internally by `apply_timeout_overrides` and `nats_connect_timeout`.

Timeout overrides require integer values ≥ 1; invalid or sub-minimum values log a warning and keep the prior/default value.

### Shared NATS connection env (`trogon_nats`)

Binaries that call `NatsConfig::from_env` honor **`trogon_nats`** auth and URL variables (priority order for auth):

| Variable | Required | Default | Meaning |
|----------|----------|---------|---------|
| `NATS_URL` | no | `localhost:4222` | Comma-separated server list |
| `NATS_CREDS` | no | — | Credentials file path (highest auth priority) |
| `NATS_NKEY` | no | — | NKey seed |
| `NATS_USER` / `NATS_PASSWORD` | no | — | User/password pair |
| `NATS_TOKEN` | no | — | Token auth |

---

## Per-binary reference

### `a2a-nats-agent`

NATS agent bridge: subscribes on `{prefix}.agent.{agent_id}.*`, provisions JetStream streams on startup (`provision_streams`).

| Variable | Required | Default | Meaning |
|----------|----------|---------|---------|
| `A2A_AGENT_ID` | **yes** | — | Agent identity segment in NATS subjects |
| `A2A_PREFIX` | no | `a2a` | Subject prefix (`ENV_A2A_PREFIX`) |
| `NATS_URL` | no | `localhost:4222` | NATS servers (`NatsConfig::from_env`) |
| `NATS_CREDS` / `NATS_NKEY` / `NATS_USER` / `NATS_PASSWORD` / `NATS_TOKEN` | no | — | NATS auth (see above) |
| `A2A_OPERATION_TIMEOUT_SECS` | no | 30 | Via `apply_timeout_overrides` |
| `A2A_TASK_TIMEOUT_SECS` | no | 7200 | Via `apply_timeout_overrides` |
| `A2A_MAX_CONCURRENT_CLIENT_TASKS` | no | 256 | Via `apply_timeout_overrides` |
| `A2A_PUSH_DLQ_CALLER_SEGMENT` | no | `_` | Via `apply_timeout_overrides` |
| `A2A_CONNECT_TIMEOUT_SECS` | no | 10 | Via `nats_connect_timeout` |

Source: [`a2a-nats-agent/src/runtime.rs`](../../rsworkspace/crates/a2a-nats-agent/src/runtime.rs).

---

### `a2a-nats-server`

HTTP JSON-RPC (and SSE streaming) front-end over `a2a_nats::Client`. Full run instructions and route notes: [`a2a-nats-server/README.md`](../../rsworkspace/crates/a2a-nats-server/README.md).

| Variable | Required | Default | Meaning |
|----------|----------|---------|---------|
| `A2A_AGENT_ID` | **yes** | — | Target agent for outbound NATS RPC |
| `A2A_PREFIX` | no | `a2a` | Subject prefix |
| `A2A_HTTP_BIND` | no | `0.0.0.0:8080` | TCP listen address (`ENV_HTTP_BIND` in server runtime) |
| `A2A_USE_GATEWAY` | no | off | Truthy (`1`, `true`, `yes`, `on`) routes unary traffic via `{prefix}.gateway.{agent_id}.*` for `a2a-gateway` |
| `NATS_URL` | no | `localhost:4222` | NATS servers |
| `NATS_CREDS` / `NATS_NKEY` / `NATS_USER` / `NATS_PASSWORD` / `NATS_TOKEN` | no | — | NATS auth |
| `A2A_OPERATION_TIMEOUT_SECS` | no | 30 | Via `apply_timeout_overrides` |
| `A2A_TASK_TIMEOUT_SECS` | no | 7200 | Via `apply_timeout_overrides` |
| `A2A_MAX_CONCURRENT_CLIENT_TASKS` | no | 256 | Via `apply_timeout_overrides` |
| `A2A_PUSH_DLQ_CALLER_SEGMENT` | no | `_` | Via `apply_timeout_overrides` |
| `A2A_CONNECT_TIMEOUT_SECS` | no | 10 | Via `nats_connect_timeout` |

Source: [`a2a-nats-server/src/runtime.rs`](../../rsworkspace/crates/a2a-nats-server/src/runtime.rs).

---

### `a2a-nats-stdio`

Line-delimited JSON-RPC over stdin/stdout using `a2a_nats::Client` (no HTTP).

| Variable | Required | Default | Meaning |
|----------|----------|---------|---------|
| `A2A_AGENT_ID` | **yes** | — | Target agent for outbound NATS RPC |
| `A2A_PREFIX` | no | `a2a` | Subject prefix |
| `NATS_URL` | no | `localhost:4222` | NATS servers |
| `NATS_CREDS` / `NATS_NKEY` / `NATS_USER` / `NATS_PASSWORD` / `NATS_TOKEN` | no | — | NATS auth |
| `A2A_OPERATION_TIMEOUT_SECS` | no | 30 | Via `apply_timeout_overrides` |
| `A2A_TASK_TIMEOUT_SECS` | no | 7200 | Via `apply_timeout_overrides` |
| `A2A_MAX_CONCURRENT_CLIENT_TASKS` | no | 256 | Via `apply_timeout_overrides` |
| `A2A_PUSH_DLQ_CALLER_SEGMENT` | no | `_` | Via `apply_timeout_overrides` |
| `A2A_CONNECT_TIMEOUT_SECS` | no | 10 | Via `nats_connect_timeout` |

Source: [`a2a-nats-stdio/src/runtime.rs`](../../rsworkspace/crates/a2a-nats-stdio/src/runtime.rs).

---

### `a2a-nats-discovery`

Agent catalog discovery and registrar over NATS KV. Configuration is via **clap `Args`** (env-backed long flags).

| Flag / env | Required | Default | Meaning |
|------------|----------|---------|---------|
| `--nats-url` / `NATS_URL` | no | `localhost:4222` | Comma-separated NATS URLs (overrides `NATS_URL` in `NatsConfig` when set on CLI) |
| `--prefix` / `A2A_PREFIX` | no | `a2a` | Subject prefix for `{prefix}.discover.*` |
| `NATS_CREDS` / `NATS_NKEY` / `NATS_USER` / `NATS_PASSWORD` / `NATS_TOKEN` | no | — | NATS auth via `NatsConfig::from_env` |
| `A2A_CONNECT_TIMEOUT_SECS` | no | 10 | Via `nats_connect_timeout` (not exposed as CLI flag) |

Source: [`a2a-nats-discovery/src/config.rs`](../../rsworkspace/crates/a2a-nats-discovery/src/config.rs), [`runtime.rs`](../../rsworkspace/crates/a2a-nats-discovery/src/runtime.rs).

---

### `a2a-gateway`

Subscribes on `{prefix}.gateway.>` and forwards ingress to mapped `{prefix}.agent.{agent_id}.*` subjects.

| Flag / env | Required | Default | Meaning |
|------------|----------|---------|---------|
| `--nats-url` / `NATS_URL` | no | `localhost:4222` | Comma-separated NATS URLs |
| `--prefix` / `A2A_PREFIX` | no | `a2a` | A2A subject prefix |
| `--queue-group` / `A2A_GATEWAY_QUEUE_GROUP` | no | — | Optional NATS queue group for gateway subscribers; unset ⇒ ephemeral subscriber |
| `NATS_CREDS` / `NATS_NKEY` / `NATS_USER` / `NATS_PASSWORD` / `NATS_TOKEN` | no | — | NATS auth via `NatsConfig::from_env` |
| `A2A_CONNECT_TIMEOUT_SECS` | no | 10 | Via `nats_connect_timeout` |

CLI/env wiring: [`a2a-gateway/src/config.rs`](../../rsworkspace/crates/a2a-gateway/src/config.rs). Runtime: [`a2a-gateway/src/runtime.rs`](../../rsworkspace/crates/a2a-gateway/src/runtime.rs).

---

## Related docs

- [Documentation index](./A2A_DOCS_INDEX.md)
- [Push DLQ operations](./A2A_PUSH_DLQ_OPS.md)
- [NSC account bootstrap](./A2A_NSC_ACCOUNT_BOOTSTRAP.md)
- [Architecture plan](../A2A_PLAN.md)
- [Open work tracker](../A2A_TODO.md)
