# SpiceDB provisioning for agentgateway v0.1

Tier-1 authorization for MCP `tools/call`, `resources/read`, and A2A
`message/{send,stream}` plus task lifecycle is enforced by SpiceDB
relationship checks.

## Apply schema

```sh
zed import \
  --endpoint "$SPICEDB_ENDPOINT" \
  --token "$SPICEDB_PRESHARED_KEY" \
  devops/spicedb/schema.zed
```

Use `--insecure` against a local `spicedb serve` running plaintext.

## Write relationships

```sh
# Allow principal `user:alice` to call MCP tool `weather`.
zed relationship create trogon/mcp_tool:weather caller trogon/principal:alice

# Allow team `eng` to invoke A2A agent `support-bot`.
zed relationship create trogon/a2a_agent:support-bot invoker trogon/group:eng#member
```

Gateway env var alignment lives in
`rsworkspace/crates/trogon-mcp-gateway/README.md`.
