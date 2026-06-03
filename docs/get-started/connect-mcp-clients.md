# Connect MCP clients (Claude Code, Cursor, VS Code, …)

Every MCP client that supports stdio transport can be wired to the
Trogon gateway via the `mcp-nats-stdio` bridge. The bridge runs as a
local process that the client invokes; it translates stdio JSON-RPC
into NATS request/reply against `mcp.gateway.request.<server_id>`.

## Prerequisites

- Gateway deployed (`docs/get-started/deploy-agentgateway.md`).
- `mcp-nats-stdio` binary available locally
  (`cargo install --path rsworkspace/crates/mcp-nats-stdio` or download
  from the release tarball).
- A NATS user credential (`.creds`) for your account.

## Claude Code

Add to `~/.claude.json` (per-project or per-user):

```jsonc
{
  "mcpServers": {
    "trogon-weather": {
      "command": "mcp-nats-stdio",
      "args": ["--server-id", "weather", "--nats-url", "nats://nats.example.com:4222", "--creds", "/secrets/user.creds"]
    }
  }
}
```

## Cursor

`~/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "trogon-weather": {
      "command": "mcp-nats-stdio",
      "args": ["--server-id", "weather", "--nats-url", "nats://nats.example.com:4222", "--creds", "/secrets/user.creds"]
    }
  }
}
```

## VS Code (Continue extension)

`~/.continue/config.json`:

```json
{
  "mcpServers": [
    {
      "name": "trogon-weather",
      "transport": "stdio",
      "command": "mcp-nats-stdio",
      "args": ["--server-id", "weather", "--nats-url", "nats://nats.example.com:4222", "--creds", "/secrets/user.creds"]
    }
  ]
}
```

## Windsurf, GitHub Copilot, others

Any MCP client that takes a `command` + `args` for a stdio MCP server
works with `mcp-nats-stdio`. The flags are stable:

| Flag | Purpose |
|---|---|
| `--server-id <id>` | Maps to `mcp.gateway.request.<id>` ingress subject. |
| `--nats-url <url>` | NATS endpoint. |
| `--creds <path>` | NSC user credentials. |
| `--bearer-token <jwt>` | Optional JWT to attach as the `authorization` header. |
| `--tenant <id>` | Optional tenant header (`trogon-mcp-tenant`). |

## Observe the call path

```sh
nats stream view MCP_AUDIT --filter mcp.audit.<server_id>
```

Each MCP tool call from the client lands as an audit envelope with the
caller ID resolved from the JWT.
