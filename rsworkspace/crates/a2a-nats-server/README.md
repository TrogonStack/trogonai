# a2a-nats-server

Axum HTTP adapter that exposes A2A JSON-RPC (and SSE for streaming) via the in-tree **`a2a_nats::Client`**. Targets local agents behind NATS; this is **not** the future HTTPS cross-binding bridge (**`a2a-bridge`**, see [A2A architecture](../../../docs/a2a/explanation/architecture.md)).

## Run

From `rsworkspace/`:

```bash
export A2A_AGENT_ID=my-agent
cargo run -p a2a-nats-server
```

## Environment

| Variable | Required | Default | Meaning |
|----------|----------|---------|---------|
| **`A2A_AGENT_ID`** | **yes** | — | Deployed NATS **`agent_id`** (`{prefix}.agent.{agent_id}.…`). |
| **`NATS_URL`** | no | _(NATS tooling default)_ | Comma-separated URLs (`trogon_nats::NatsConfig`). |
| **`A2A_PREFIX`** | no | **`a2a`** | Subject prefix. |
| **`A2A_HTTP_BIND`** | no | **`0.0.0.0:8080`** | TCP listen address. |
| **`A2A_OPERATION_TIMEOUT_SECS`** | no | _(library)_ | Unary JSON-RPC timeouts (`a2a_nats` constant). |
| **`A2A_TASK_TIMEOUT_SECS`** | no | _(library)_ | Streaming-related timeouts. |
| **`A2A_CONNECT_TIMEOUT_SECS`** | no | _(library)_ | NATS dial timeout. |
| **`A2A_MAX_CONCURRENT_CLIENT_TASKS`** | no | **`256`** | `Bridge` concurrent streaming task semaphore (`apply_timeout_overrides`). **`0`** normalizes to **`1`**. |
| **`A2A_PUSH_DLQ_CALLER_SEGMENT`** | no | **`_`** | Push DLQ **`{caller_id}`** subject segment (`apply_timeout_overrides`). |
| **`A2A_USE_GATEWAY`** | no | off | Truthy (**`1`**, **`true`**, **`yes`**, **`on`**) ⇒ unary / bootstrap publishes on **`{prefix}.gateway.{agent_id}.{method}`** for **`a2a-gateway`** to forward. Tenancy follows the NATS **`Connection`/`Account`** (see [A2A architecture §Decisions](../../../docs/a2a/explanation/architecture.md)); there is **no `A2A_TENANT` subject knob**. |
| **`A2A_GATEWAY_CALLER_JWT`** | when gateway on | — | Auth-callout-minted User JWT for **`A2a-Caller-Jwt`** on gateway publishes (usually the same JWT as **`NATS_CREDS`**). |

Ingress auth/policy/audit hooks are **`[P]`** (see [A2A architecture §Decisions](../../../docs/a2a/explanation/architecture.md)).

## Routes

Serve JSON-RPC **`POST`** on **`/`** (`application/json`); streaming SSE paths are wired in **`src/router.rs`** and **`src/handlers`**.

## Related

- [A2A architecture](../../../docs/a2a/explanation/architecture.md)
- [`a2a-gateway`](../a2a-gateway/README.md)
