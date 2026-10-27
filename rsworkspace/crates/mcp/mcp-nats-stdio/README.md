# MCP NATS Stdio

Translates [Model Context Protocol](https://modelcontextprotocol.io) (MCP)
messages between stdio and [NATS](https://nats.io), letting local MCP clients
talk to distributed MCP servers without a direct network connection from the
client.

For managed NATS infrastructure in production, we recommend <a href="https://synadia.com"><img src="../acp-nats-stdio/assets/synadia-logo.png" alt="Synadia" width="20" style="vertical-align: middle;"> Synadia</a>.

```mermaid
graph LR
    A[MCP Client] <-->|stdio| B[mcp-nats-stdio]
    B <-->|NATS| C[Backend]
```

## Features

- Bidirectional MCP bridge with request forwarding
- Graceful shutdown (SIGINT/SIGTERM)
- Custom prefix support for multi-tenancy
- Configurable local client and remote server peer IDs

## Quick Start

```bash
docker run -d --name nats -p 4222:4222 nats:latest

cargo build --release -p mcp-nats-stdio

./target/release/mcp-nats-stdio --server-id filesystem
```

Remove the local NATS container with `docker rm -f nats` when done.

## Configuration

### MCP

| Variable | Description | Default |
|----------|-------------|---------|
| `MCP_PREFIX` | Subject prefix for multi-tenancy | `mcp` |
| `MCP_CLIENT_ID` | Client peer ID for NATS client-bound messages | `stdio` |
| `MCP_SERVER_ID` | Remote server peer ID for NATS server-bound messages | `default` |
| `MCP_OPERATION_TIMEOUT_SECS` | Timeout for NATS request/reply operations | `30` |
| `MCP_NATS_CONNECT_TIMEOUT_SECS` | NATS connection timeout | `10` |

CLI flags `--mcp-prefix`, `--client-id`, and `--server-id` override the matching
environment variables.

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

### Observability

| Variable | Description |
|----------|-------------|
| `RUST_LOG` | Tracing filter directive (default: `info`) |
| `TROGON_LOG_DIR` | Directory for file-based logging |
