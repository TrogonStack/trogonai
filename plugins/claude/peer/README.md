# Trogon peer plugin for Claude Code

This plugin connects a normal Claude Code session to the peer service through a
local stdio MCP connector. It requires `trogon-peer-mcp` on `PATH` and authorized
service access configured before launch. Keep credentials outside this plugin.

The connector inherits the process environment. Set the nonsecret configuration
explicitly before starting the host:

| Setting | Meaning |
| --- | --- |
| `PEER_TENANT_ID` | Tenant identity, required in either routing mode |
| `PEER_ACCOUNT_MODE` | `tenant` or the explicit test-only `shared_fixture` mode |
| `PEER_CALLER_ID` | Caller token fixed by the broker-issued credential grants |
| `PEER_CONNECTION_ID` | Connection token fixed by those grants |
| `PEER_INSTALLATION_ID` | Persistent identity retained across connector restarts |
| `PEER_COLLABORATION_REF` | Collaboration visible to the authenticated owner |
| `PEER_DISPLAY_NAME` | Human-readable peer name |
| `PEER_AVAILABILITY` | Explicit observation: `ready`, `busy`, `blocked`, or `draining` |

Configure the shared `NATS_URL`, authentication and TLS settings as documented in
[the connector reference](../../../rsworkspace/crates/mcp/trogon-peer-mcp/README.md).
The executable refuses anonymous authentication. TLS is on by default. Tenant
account subjects omit the tenant prefix but still require `PEER_TENANT_ID`.
Caller and connection settings must match actual broker-issued grants. Settings
do not confer authority. Each runtime generates its own connector identity;
that identity is separate from both the credential connection token and the
persistent installation identity.

The Unix executable has local authenticated TLS/JetStream subprocess coverage.
Actual installed-host callback and context insertion remain separate acceptance
work. Plugin validation checks package shape only.

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
completion. A changed native session requires a new connector process;
in-process conversation switching is outside this adapter's first proof.

See the host's [MCP hook reference](https://code.claude.com/docs/en/hooks#mcp-tool-hook-fields)
and [plugin manifest reference](https://code.claude.com/docs/en/plugins-reference).
