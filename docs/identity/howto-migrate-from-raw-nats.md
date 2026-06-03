# Migrate from raw NATS request/reply

If your existing agent integration uses raw NATS request/reply against
agent subjects directly, this how-to walks the diff to put the
agentgateway in front of those subjects.

## What changes

| Before | After |
|---|---|
| Clients publish to `mcp.server.<id>.<method>` | Clients publish to `mcp.gateway.request.<id>` (or `a2a.gateway.<agent_id>.<…>`) |
| Servers subscribe directly | Servers subscribe to the same subjects; the gateway rewrites ingress → egress |
| No authz | JWT validation + Tier-1 SpiceDB |
| No audit | Audit envelopes on `MCP_AUDIT` / `A2A_AUDIT` |
| Caller identity = NATS account | Caller identity = JWT `sub` / `aud` + tenant header |

## Step 1 — name your subjects

Pick a prefix (default `mcp`). Existing direct subjects become
**egress** subjects; the gateway's ingress is the front door.

- Egress: `mcp.server.<id>.<method>` (unchanged).
- Ingress: `mcp.gateway.request.<id>` (new, used by clients).

## Step 2 — install the gateway

Follow `docs/get-started/deploy-agentgateway.md`. Set
`MCP_GATEWAY_MCP_PREFIX` to your prefix.

## Step 3 — repoint clients

Replace publishes to `mcp.server.<id>.<method>` with publishes to
`mcp.gateway.request.<id>` and the JSON-RPC `method` carried in the
request body. The gateway rewrites the egress subject.

## Step 4 — wire identity

Mint JWTs (via `trogon-sts` or your own issuer) and attach them on
the `authorization` header. Configure `MCP_GATEWAY_JWT_ISSUERS` and
`MCP_GATEWAY_JWT_JWKS_URI` on the gateway.

## Step 5 — write Tier-1 SpiceDB rules

Pick `trogon/mcp_tool:<tool>` resources and grant `caller` relations
for each principal. See `devops/spicedb/schema.zed`.

## Step 6 — observe

Tail `MCP_AUDIT` to see allow/deny decisions per call. Wire
`TROGON_OTEL_ENDPOINT` for traces.

## Roll-forward

There is no hard breakage: existing servers continue to listen on
their original subjects. The gateway only re-routes **client**
traffic. Roll your clients release-by-release.
