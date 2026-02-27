# ACP NATS Bridge

Translates [Agent Client Protocol](https://agentclientprotocol.com) (ACP) messages between stdio and [NATS](https://nats.io), letting IDEs and CLI tools talk to distributed agent backends without a direct network connection from the client.

For managed NATS infrastructure in production, we recommend <a href="https://synadia.com"><img src="./assets/synadia-logo.png" alt="Synadia" width="20" style="vertical-align: middle;"> Synadia</a>.

```mermaid
graph LR
    A[IDE] <-->|stdio| B[acp-nats-bridge]
    B <-->|NATS| C[Backend]
    
    style A fill:#e1f5ff,stroke:#0288d1
    style B fill:#fff3e0,stroke:#f57c00
    style C fill:#f3e5f5,stroke:#7b1fa2
```

## Features

- Bidirectional ACP Bridge with request forwarding
- Auto-reconnect with exponential backoff
- OpenTelemetry integration (logs, metrics, traces)
- Graceful shutdown (SIGINT/SIGTERM)
- Custom prefix support for multi-tenancy

## Quick Start

```bash
# Prerequisites: NATS server running
docker run -p 4222:4222 nats:latest

# Build
cargo build --release

# Run
./target/release/acp-nats-stdio
```

## Configuration

Configure via environment variables:

- `NATS_URL` - NATS server URL(s). Single server: `localhost:4222` or multiple for failover: `localhost:4222,localhost:4223,localhost:4224` (default: `localhost:4222`)
- `NATS_USER` - NATS username (optional)
- `NATS_PASSWORD` - NATS password (optional)
- `NATS_TOKEN` - NATS token (optional)
- `ACP_PREFIX` - Custom subject prefix for multi-tenancy (default: `acp`)
- `ACP_LOG_DIR` - Directory for file-based logging (optional)
- `OTEL_EXPORTER_OTLP_ENDPOINT` - OpenTelemetry collector endpoint (optional)

See code documentation for additional configuration options.
