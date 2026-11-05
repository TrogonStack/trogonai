# a2a-nats-http

Axum HTTP adapter that exposes A2A JSON-RPC (and SSE for streaming) via the in-tree **`a2a_nats::A2aClient`**. Targets local agents behind NATS; this is **not** the future HTTPS cross-binding bridge (**`a2a-bridge`**, see [A2A architecture](../../../../docs/a2a/explanation/architecture.md)).

## Run

From `rsworkspace/`:

```bash
export A2A_AGENT_ID=my-agent
export A2A_CALLER_JWT="$(cat caller.jwt)"
cargo run -p a2a-nats-http
```

## Environment

| Variable | Required | Default | Meaning |
|----------|----------|---------|---------|
| **`A2A_AGENT_ID`** | **yes** | none | Deployed NATS **`agent_id`** (`{prefix}.v1.agents.{agent_id}.{kind}.{id}.…`). |
| **`A2A_CALLER_JWT`** | **yes** | none | Auth-callout-minted User JWT for the caller this adapter acts as (usually the same JWT as **`NATS_CREDS`**). Its **`principal_kind`** and **`caller_id`** claims fill the **`{kind}.{id}`** principal position of every subject the adapter publishes or subscribes to, so the token must be unexpired and name a principal. Sent as **`A2a-Caller-Jwt`** only on gateway publishes. |
| **`NATS_URL`** | no | _(NATS tooling default)_ | Comma-separated URLs (`trogon_nats::NatsConfig`). |
| **`NATS_USER`**, **`NATS_PASSWORD`**, **`NATS_TOKEN`**, **`NATS_CREDS`**, **`NATS_NKEY`**, **`NATS_JWT`**, **`NATS_SEED`**, **`NATS_CERT`**, **`NATS_KEY`**, **`NATS_CA`** | no | none | Accept the same values as the `nats` CLI. First match wins: credentials file, then NKey (`NATS_NKEY` is a seed or a seed file path), then user/password, then token (`NATS_USER` without a password counts as the token), then JWT (signed with `NATS_SEED` when set). Secret values accept `file://` and `env://`; TLS needs `NATS_CERT` with `NATS_KEY`, or `NATS_CA`. |
| **`A2A_PREFIX`** | no | **`a2a`** | Subject prefix. |
| **`A2A_HTTP_BIND`** | no | **`0.0.0.0:8080`** | TCP listen address. |
| **`A2A_OPERATION_TIMEOUT_SECS`** | no | _(library)_ | Unary JSON-RPC timeouts (`a2a_nats` constant). |
| **`A2A_TASK_TIMEOUT_SECS`** | no | _(library)_ | Streaming-related timeouts. |
| **`A2A_CONNECT_TIMEOUT_SECS`** | no | _(library)_ | NATS dial timeout. |
| **`A2A_MAX_CONCURRENT_CLIENT_TASKS`** | no | **`256`** | `Bridge` concurrent streaming task semaphore (`apply_timeout_overrides`). **`0`** normalizes to **`1`**. |
| **`A2A_PUSH_DLQ_CALLER_SEGMENT`** | no | **`_`** | Push DLQ **`{caller_id}`** subject segment (`apply_timeout_overrides`). |
| **`A2A_USE_GATEWAY`** | no | off | Truthy (**`1`**, **`true`**, **`yes`**, **`on`**) ⇒ unary / bootstrap publishes on **`{prefix}.v1.gateway.{agent_id}.{kind}.{id}.{method}`** for **`a2a-gateway`** to forward. Tenancy follows the NATS **`Connection`/`Account`** (see [A2A architecture §Decisions](../../../../docs/a2a/explanation/architecture.md)); there is **no `A2A_TENANT` subject knob**. |

Ingress auth/policy/audit hooks are **`[P]`** (see [A2A architecture §Decisions](../../../../docs/a2a/explanation/architecture.md)).

## Routes

Serve JSON-RPC **`POST`** on **`/`** (`application/json`); streaming SSE paths are wired in **`src/router.rs`** and **`src/handlers`**.

## Related

- [A2A architecture](../../../../docs/a2a/explanation/architecture.md)
- [`a2a-gateway`](../a2a-gateway/README.md)
