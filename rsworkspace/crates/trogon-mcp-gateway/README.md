# trogon-mcp-gateway

Operators run this service so MCP JSON-RPC crosses a single NATS chokepoint (`gateway.request`) before backends under `server.*`.

## Smoke

1. Start NATS with JetStream if you audit (stream creation is optional).
2. Set `trogon-nats` / MCP env (servers, TLS, prefixes) the same way as other MCP bridges.
3. Run `trogon-mcp-gateway`. Use `MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT=1` during quick tests to skip `get_or_create_stream`.

Clients that already targeted `{prefix}.server.{id}.{method}` must instead publish (or NATS-request) onto `{prefix}.gateway.request.{id}.{method}` — the gateway rewrites to the server lane and preserves inbox reply semantics when a reply inbox is attached.

Optional header **`trogon-mcp-tenant`** seeds the tenant field in JetStream audit JSON.

## Tune

| Variable | Meaning |
|---------|---------|
| `MCP_GATEWAY_QUEUE_GROUP` | Queue group for HA workers (default `mcp-gateway`) |
| `MCP_GATEWAY_AUDIT_STREAM` | JetStream stream name (default `MCP_AUDIT`) |
| `MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT` | Truthy ⇒ skip bootstrap `get_or_create_stream` |

## Integration checks

Skipped by default (`#[ignore]`). With a reachable broker:

```bash
export NATS_URL=nats://127.0.0.1:4222
cargo test -p trogon-mcp-gateway -- --ignored
```
