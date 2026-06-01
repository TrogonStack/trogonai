# NATS provisioning for agentgateway v0.1

Apply with `nats` CLI (>=0.1.5) against a JetStream-enabled cluster.

```sh
# Streams
nats stream add --config devops/nats/streams/mcp-audit.json
nats stream add --config devops/nats/streams/a2a-audit.json
nats stream add --config devops/nats/streams/sts-audit.json
nats stream add --config devops/nats/streams/registry-audit.json
nats stream add --config devops/nats/streams/approvals.json

# KV buckets
nats kv add --config devops/nats/kv/trogon-claims.json
nats kv add --config devops/nats/kv/agent-registry.json
nats kv add --config devops/nats/kv/policy-bundle.json
```

The default prefix is `mcp`. Override with `MCP_GATEWAY_MCP_PREFIX`,
`MCP_STS_PREFIX`, `MCP_REGISTRY_PREFIX` to deploy under an alternate
namespace; replace the `mcp.*` subjects in each JSON accordingly.

## Account / auth-callout (v0.2)

v0.1 ships with the stub `a2a-bridge` auth callout. The decentralized
NATS `$SYS` auth-callout integration is tracked in
`docs/roadmap/agentgateway-v0.2.md` and is intentionally out of scope
for this release.
