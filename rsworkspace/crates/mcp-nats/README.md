# mcp-nats

`mcp-nats` routes Model Context Protocol JSON-RPC messages over NATS using the
official Rust MCP SDK (`rmcp`) model and transport traits.

## Subject Layout

Subjects mirror the method-oriented shape used by `acp-nats`, with the MCP role
and peer ID before the method suffix.

Server-bound messages:

```text
mcp.server.{server_id}.initialize
mcp.server.{server_id}.tools.list
mcp.server.{server_id}.tools.call
mcp.server.{server_id}.resources.read
mcp.server.{server_id}.notifications.initialized
```

Client-bound messages:

```text
mcp.client.{client_id}.sampling.create_message
mcp.client.{client_id}.roots.list
mcp.client.{client_id}.elicitation.create
mcp.client.{client_id}.notifications.tools.list_changed
```

Wildcard subscriptions:

```text
mcp.server.>
mcp.server.{server_id}.>
mcp.client.>
mcp.client.{client_id}.>
```

## Transport

Use `client::connect` for a local MCP client that talks to a remote MCP server
over NATS. Use `server::connect` for a local MCP server that talks to a remote
MCP client over NATS.

```rust,no_run
use mcp_nats::{Config, McpPeerId, McpPrefix, client};
use rmcp::ServiceExt;

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let config = Config::new(
    McpPrefix::new("mcp")?,
    mcp_nats::NatsConfig {
        servers: vec!["localhost:4222".to_string()],
        auth: mcp_nats::NatsAuth::None,
    },
);
# let nats = mcp_nats::nats::connect(config.nats(), std::time::Duration::from_secs(10)).await?;

let transport = client::connect(
    nats,
    &config,
    McpPeerId::new("desktop")?,
    McpPeerId::new("filesystem")?,
)
.await?;

let running = ().serve(transport).await?;
# Ok(())
# }
```

## Environment

| Variable | Purpose | Default |
| --- | --- | --- |
| `MCP_PREFIX` | NATS subject prefix | `mcp` |
| `MCP_OPERATION_TIMEOUT_SECS` | NATS request/reply timeout | `30` |
| `MCP_NATS_CONNECT_TIMEOUT_SECS` | NATS connect timeout | `10` |
| `NATS_URL` | NATS server URL(s) | `localhost:4222` |

NATS authentication is loaded through [`trogon-nats`](../trogon-nats/README.md)
using the same environment variables as the rest of the workspace.

## Stdio Bridge

Use `mcp-nats-stdio` when a local MCP client expects to launch an MCP server
process over stdio, but the target MCP server is reachable through NATS.

```sh
mise exec -- cargo run -p mcp-nats-stdio -- --server-id filesystem
```

## Testing

Run the crate checks with:

```sh
mise exec -- cargo test -p mcp-nats
mise exec -- cargo clippy -p mcp-nats --all-targets
```
