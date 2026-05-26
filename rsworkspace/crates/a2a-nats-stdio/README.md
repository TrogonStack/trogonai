# a2a-nats-stdio

Line-delimited **JSON-RPC 2.0** over **stdin/stdout**, forwarding A2A methods to a NATS agent via [`a2a_nats::Client`](../a2a-nats/src/client/mod.rs). Useful for MCP-style host processes, scripts, and other embedders that want A2A without HTTP.

The **`a2a-nats-stdio`** binary reads one JSON-RPC request per line, dispatches through [`dispatch`](../a2a-nats-stdio/src/dispatch.rs) (`message/send`, `tasks/*`, push-notification config, and related streaming notifications), and writes one JSON line per response or stream event to stdout.

The crate also exposes [`run`](./src/runtime.rs) as a library entry point for custom I/O or tests.

## Run

From `rsworkspace/`:

```bash
export A2A_AGENT_ID=demo-agent
cargo run -p a2a-nats-stdio
```

Send requests on stdin (one JSON object per line), for example:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"tasks/list","params":{}}' | \
  A2A_AGENT_ID=demo-agent cargo run -p a2a-nats-stdio
```

## Environment

Required and shared knobs mirror [`docs/a2a/reference/runtime-env.md`](../../../docs/a2a/reference/runtime-env.md) (**`a2a-nats-stdio`** section). At minimum, set **`A2A_AGENT_ID`**; NATS URL/auth and timeout overrides follow the shared **`a2a_nats`** / **`trogon_nats`** conventions documented there.

**Navigation hub:** [`docs/a2a/README.md`](../../../docs/a2a/README.md).
