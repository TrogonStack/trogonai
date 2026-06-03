# NATS provisioning for agentgateway v0.1

Apply with `nats` CLI (>=0.1.5) against a JetStream-enabled cluster.

```sh
# Streams
nats stream add --config devops/nats/streams/mcp-audit.json
nats stream add --config devops/nats/streams/a2a-audit.json
nats stream add --config devops/nats/streams/sts-audit.json
nats stream add --config devops/nats/streams/registry-audit.json
nats stream add --config devops/nats/streams/approvals.json
nats stream add --config devops/nats/streams/a2a-events.json
nats stream add --config devops/nats/streams/a2a-push-dlq.json

# KV buckets
nats kv add --config devops/nats/kv/trogon-claims.json
nats kv add --config devops/nats/kv/agent-registry.json
nats kv add --config devops/nats/kv/policy-bundle.json
nats kv add --config devops/nats/kv/mcp-sessions.json
nats kv add --config devops/nats/kv/mcp-trust-bundles.json
nats kv add --config devops/nats/kv/mcp-gateway-config.json
nats kv add --config devops/nats/kv/mcp-jwks.json
nats kv add --config devops/nats/kv/a2a-catalog.json
```

The default prefix is `mcp`. Override with `MCP_GATEWAY_MCP_PREFIX`,
`MCP_STS_PREFIX`, `MCP_REGISTRY_PREFIX` to deploy under an alternate
namespace; replace the `mcp.*` subjects in each JSON accordingly.

## NATS server config + auth callout

`nats-server.example.conf` is a paste-ready reference config with
JetStream, decentralized operator-mode auth, the system account, and
the `$SYS.REQ.USER.AUTH` callout entry pointing at the
`a2a-auth-callout` service. Replace every `<…>` placeholder before
applying.

For the operator / Account JWT story (operator key, Account JWT,
resolver setup), see `devops/nsc/README.md`. Service ACL templates
mirroring `scripts/acl-templates/` are duplicated under
`devops/nsc/users/` so a release tarball is self-contained.

## SpiceDB

Tier-1 authorization runs against SpiceDB. Reference schema and
relationship examples live in `devops/spicedb/`.

## Out of scope

- `acp-sessions` KV — ACP today is subject-only request/reply. No code
  path reads or writes a session bucket; no manifest is shipped. Revisit
  if ACP session resume across pods becomes a v0.2 requirement.
- `MCP_DECISIONS` stream — the `trogon-traffic-view` projector accepts
  a configurable audit stream name and consumes the existing
  `MCP_AUDIT` / `A2A_AUDIT` streams. No separate decisions stream
  manifest is required for v0.1.
