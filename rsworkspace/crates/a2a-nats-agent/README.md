# a2a-nats-agent

Process that connects to NATS, **provisions** per-Account **`A2A_EVENTS`** + **`A2A_PUSH_DLQ`** via [`provision_streams`](../a2a-nats/src/jetstream/provision.rs), and runs the agent-side [`Bridge`](../a2a-nats/src/agent/bridge.rs) on **`{prefix}.agent.{agent_id}.>`**.

The default **`a2a-nats-agent`** binary installs a **`NoopHandler`** (every method responds *unsupported*) so you can verify connectivity before wiring your own **`A2aHandler`**.

## Run

From `rsworkspace/`:

```bash
export A2A_AGENT_ID=demo-agent
cargo run -p a2a-nats-agent
```

Echo-style sample handler:

```bash
cargo run -p a2a-nats-agent --example echo
```

See also [`examples/echo.rs`](./examples/echo.rs).

## Environment

Required and shared knobs mirror [`docs/A2A_RUNTIME_ENV.md`](../../../docs/A2A_RUNTIME_ENV.md) (**`a2a-nats-agent`** section).

| Variable | Required | Meaning |
|----------|----------|---------|
| **`A2A_AGENT_ID`** | **yes** | NATS subject segment **`{prefix}.agent.{A2A_AGENT_ID}.…`** |
| **`A2A_PREFIX`** | no | Prefix (default **`a2a`**) |
| **`NATS_URL`** / **`NATS_*`** auth | no | **`trogon_nats::NatsConfig::from_env`** |
| **`A2A_OPERATION_TIMEOUT_SECS`** | no | Unary RPC budget (`apply_timeout_overrides`) |
| **`A2A_TASK_TIMEOUT_SECS`** | no | Streaming task budget |
| **`A2A_MAX_CONCURRENT_CLIENT_TASKS`** | no | **`Bridge`** streaming semaphore (**`0` → 1**) |
| **`A2A_PUSH_DLQ_CALLER_SEGMENT`** | no | Push DLQ **`{caller_id}`** segment (default **`_`**) |
| **`A2A_CONNECT_TIMEOUT_SECS`** | no | NATS dial timeout |

**Navigation hub:** [`docs/A2A_DOCS_INDEX.md`](../../../docs/A2A_DOCS_INDEX.md).
