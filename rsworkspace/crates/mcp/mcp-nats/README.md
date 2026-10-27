# MCP NATS

Routes [Model Context Protocol](https://modelcontextprotocol.io) (MCP) JSON-RPC
messages over [NATS](https://nats.io) using the official Rust MCP SDK (`rmcp`)
model and transport traits.

For managed NATS infrastructure in production, we recommend <a href="https://synadia.com"><img src="../acp-nats-stdio/assets/synadia-logo.png" alt="Synadia" width="20" style="vertical-align: middle;"> Synadia</a>.

## When to Use

- Use `mcp-nats` when embedding the NATS transport into an MCP client or server.
- Use `mcp-nats-stdio` when a local MCP client launches a stdio process.
- Use `mcp-nats-server` when MCP clients connect over Streamable HTTP.

## Subject Layout

Subjects mirror the method-oriented shape used by `acp-nats`, with a binding
version token (`v1`), the MCP role, and peer ID before the method suffix.

Server-bound messages:

```text
mcp.v1.server.{server_id}.initialize
mcp.v1.server.{server_id}.tools.list
mcp.v1.server.{server_id}.tools.call
mcp.v1.server.{server_id}.resources.read
mcp.v1.server.{server_id}.notifications.initialized
```

Client-bound messages:

```text
mcp.v1.client.{client_id}.sampling.create_message
mcp.v1.client.{client_id}.roots.list
mcp.v1.client.{client_id}.elicitation.create
mcp.v1.client.{client_id}.notifications.tools.list_changed
```

Wildcard subscriptions:

```text
mcp.v1.server.>
mcp.v1.server.{server_id}.>
mcp.v1.client.>
mcp.v1.client.{client_id}.>
```

## Transport

Use `client::connect` for a local MCP client that talks to a remote MCP server
over NATS. Use `server::connect` for a local MCP server that talks to a remote
MCP client over NATS.

```rust,no_run
use mcp_nats::{client, Config, McpPeerId, McpPrefix};
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

let _running = ().serve(transport).await?;
# Ok(())
# }
```

## Configuration

### MCP

| Variable | Description | Default |
|----------|-------------|---------|
| `MCP_PREFIX` | Subject prefix for multi-tenancy | `mcp` |
| `MCP_OPERATION_TIMEOUT_SECS` | Timeout for NATS request/reply operations | `30` |
| `MCP_NATS_CONNECT_TIMEOUT_SECS` | NATS connection timeout | `10` |

### NATS

| Variable | Description | Default |
|----------|-------------|---------|
| `NATS_URL` | Server URL(s), comma-separated for failover | `localhost:4222` |

### NATS Authentication

These variables accept the same values as the `nats` CLI. One method is chosen, first match wins:

| Priority | Variable(s) | Method |
|----------|-------------|--------|
| 1 | `NATS_CREDS` | Credentials file, a path or `file://path` |
| 2 | `NATS_NKEY` | NKey seed, or a seed or `.creds` file as a path or `file://path` |
| 3 | `NATS_USER` + `NATS_PASSWORD` | Username/password |
| 4 | `NATS_TOKEN`, or `NATS_USER` without `NATS_PASSWORD` | Token |
| 5 | `NATS_JWT` + `NATS_SEED` | User JWT signed with the seed |
| 6 | `NATS_JWT` | User JWT with an empty signature |

`NATS_JWT`, `NATS_SEED`, `NATS_PASSWORD` and `NATS_TOKEN` are literals unless they start with `file://path` (file contents, trailing newlines trimmed) or `env://NAME` (value of another variable). The `op`, `nsc` and `data` schemes are rejected at connect time.

If none are set, the connection is unauthenticated.

### NATS TLS

| Variable | Description |
|----------|-------------|
| `NATS_CERT` + `NATS_KEY` | Client certificate and key, paths or `file://path`. Setting only one of them fails the connection. |
| `NATS_CA` | Root CA certificate, a path or `file://path` |

Setting any of them requires TLS.
