# Trogon peer plugin for Claude Code

This plugin connects a normal Claude Code session to the peer service through a
local stdio MCP connector. It requires `trogon-peer-mcp` on `PATH` and authorized
service access configured before launch. Keep credentials outside this plugin.

The connector inherits the process environment. Configure the shared NATS
connection, authentication, and TLS settings there, alongside the nonsecret
`PEER_CALLER_ID`, `PEER_INSTALLATION_ID`, and `PEER_COLLABORATION_REF` settings.
`PEER_TENANT_ID` is required for both routing modes. Tenant-account routes omit
the tenant prefix from subjects while retaining the configured tenant identity. The caller setting must match the
principal admitted by the broker. It does not grant authority by itself.

The connector and service exchange is not yet verified. Plugin validation checks
configuration shape; it does not establish authentication, hook invocation, or
message delivery.

## Load for development

From the repository root:

```sh
claude plugin validate --strict --json ./plugins/claude/peer
claude --plugin-dir ./plugins/claude/peer
```

Use a host that supports native `mcp_tool` hooks. Review the server in `/mcp`
before submitting a prompt.

## Delivery behavior

`UserPromptSubmit` calls `peer_prompt_hook` with the native session ID, prompt ID
when available, and event name. The hook has a two-second timeout. A timeout lets
the prompt continue without its peer context; queued work remains available for
retry. Only selected identifiers are forwarded, never prompt text, working
directories, or transcript paths.

`SessionEnd` calls `peer_detach_hook` with a one-second budget. Detach is best
effort and preserves the durable inbox. There is no launch-time `SessionStart`
MCP hook because MCP is unavailable at that point.

Messages wait for the next prompt hook. The plugin starts no idle turn. Peer
text is untrusted data, and an application acknowledgment does not prove task
completion. A changed native session requires a new connector enrollment;
in-process conversation switching is outside this adapter's first proof.

See the host's [MCP hook reference](https://code.claude.com/docs/en/hooks#mcp-tool-hook-fields)
and [plugin manifest reference](https://code.claude.com/docs/en/plugins-reference).
