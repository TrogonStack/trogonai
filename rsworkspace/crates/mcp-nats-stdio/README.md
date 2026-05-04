# mcp-nats-stdio

Bridges Model Context Protocol JSON-RPC between stdio and NATS. The binary acts
as an MCP server on stdio for local MCP clients, and as an MCP client over NATS
for a remote MCP server.

```mermaid
graph LR
    A[MCP client] <-->|stdio| B[mcp-nats-stdio]
    B <-->|NATS| C[MCP server]
```

## Quick Start

```bash
docker run -p 4222:4222 nats:latest

cargo build --release -p mcp-nats-stdio

./target/release/mcp-nats-stdio --server-id filesystem
```

## Configuration

| Variable | Description | Default |
| --- | --- | --- |
| `MCP_PREFIX` | NATS subject prefix | `mcp` |
| `MCP_CLIENT_ID` | Client peer ID for NATS client-bound messages | `stdio` |
| `MCP_SERVER_ID` | Remote server peer ID for NATS server-bound messages | `default` |
| `MCP_OPERATION_TIMEOUT_SECS` | Timeout for NATS request/reply operations | `30` |
| `MCP_NATS_CONNECT_TIMEOUT_SECS` | NATS connection timeout | `10` |
| `NATS_URL` | NATS server URL(s), comma-separated for failover | `localhost:4222` |
| `RUST_LOG` | Tracing filter directive | `info` |

CLI flags `--mcp-prefix`, `--client-id`, and `--server-id` override the matching
environment variables.

NATS authentication is loaded through [`trogon-nats`](../trogon-nats/README.md)
using the same environment variables as the rest of the workspace.

## Testing

```sh
mise exec -- cargo test -p mcp-nats-stdio
mise exec -- cargo clippy -p mcp-nats-stdio --all-targets
```
