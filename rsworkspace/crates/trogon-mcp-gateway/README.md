# trogon-mcp-gateway

Operators run this service so MCP JSON-RPC crosses a single NATS chokepoint (`gateway.request`) before backends under `server.*`.

## Smoke

1. Start NATS with JetStream if you audit (stream creation is optional).
2. Set `trogon-nats` / MCP env (servers, TLS, prefixes) the same way as other MCP bridges.
3. Run `trogon-mcp-gateway`. Use `MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT=1` during quick tests to skip `get_or_create_stream`.

Clients that already targeted `{prefix}.server.{id}.{method}` must instead publish (or NATS-request) onto `{prefix}.gateway.request.{id}.{method}` — the gateway rewrites to the server lane and preserves inbox reply semantics when a reply inbox is attached.

Optional header **`trogon-mcp-tenant`** seeds JetStream audit JSON when no verified JWT carries a tenant claim, and still acts as the SpiceDB subject `object_id` for legacy callers when **`MCP_GATEWAY_JWT_MODE=off`** (or absent). When JWT ingress is **validate** or **require**, forgeable **`trogon-mcp-tenant`** is stripped before messages reach **`server.*`**, SpiceDB principals prefer the JWT **`sub`** (caller id), and audit/traces carry **`caller_sub`** / **`identity_source`**.

See **Verified JWT** below and align tuples in SpiceDB with gateway resource naming documented here.

## Verified JWT (`MCP_GATEWAY_JWT_*`)

| Variable | Meaning |
|---------|---------|
| `MCP_GATEWAY_JWT_MODE` | **`off`** (default), **`validate`**, or **`require`**. **`require`** enforces Bearer JWT on SpiceDB-gated methods (`tools/call`, `resources/read`). |
| `MCP_GATEWAY_JWT_ISSUERS` | Comma-separated `iss` allow-list (required when mode ≠ off). |
| `MCP_GATEWAY_JWT_AUDIENCE` | Expected `aud` (default `trogon-mcp-gateway`). |
| `MCP_GATEWAY_JWT_JWKS_URI` | HTTPS JWKS URL for RSA keys (cached ~5 minutes). |
| `MCP_GATEWAY_JWT_RSA_PUBLIC_KEY_PEM` | PEM for a single static RSA key (alternative to JWKS). |
| `MCP_GATEWAY_JWT_HS256_SECRET` | Raw secret bytes for **`HS256`** (dev/smoke paths). Configure **one or more** of JWKS / RSA PEM / HS256 when mode ≠ off. |
| `MCP_GATEWAY_JWT_LEEWAY_SECS` | Clock skew (default `60`). |
| `MCP_GATEWAY_JWT_TENANT_CLAIM` | JSON claim key for tenancy; **`tenant`** or default namespaced **`https://trogon.ai/tenant`** (falls back to plain **`tenant`**). |
| `MCP_GATEWAY_JWT_BEARER_HEADER` | Header name carrying `Bearer …` (default **`authorization`**). Matching is ASCII case-insensitive. |

`jsonwebtoken` is built with the **`rust_crypto`** backend (`Cargo.toml`).

## Tune

| Variable | Meaning |
|---------|---------|
| `MCP_GATEWAY_QUEUE_GROUP` | Queue group for HA workers (default `mcp-gateway`) |
| `MCP_GATEWAY_AUDIT_STREAM` | JetStream stream name (default `MCP_AUDIT`) |
| `MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT` | Truthy ⇒ skip bootstrap `get_or_create_stream` |

Audit payloads include **`identity_source`** (**`jwt`**, **`legacy_header`**, **`anonymous`**) plus optional **`caller_sub`** and **`jwt_issuer`** when present.

## SpiceDB (gated `tools/call` and `resources/read`)

When `MCP_GATEWAY_SPICEDB_ENDPOINT` is set (host:port without scheme, or full `http://` / `https://` URL per `spicedb-rs-client`), the gateway runs `CheckPermission` for **`tools/call`** and **`resources/read`** after a hardcoded CEL gate. Omit the variable for allow-all Phase-1 behaviour.

### Tuple shapes (defaults; override via env below)

**`tools/call`**

| Part | Default |
|------|---------|
| Resource type | `trogon/mcp_tool` |
| Resource id | Normalized `{server_id}` + `\|` + normalized `params.name` |
| Permission | `call` |

**`resources/read`**

| Part | Default |
|------|---------|
| Resource type | `trogon/mcp_resource` |
| Resource id | Normalized full `params.uri` string |
| Permission | `read` |

Both methods use the same **subject**: type `trogon/principal` (configurable), id from JWT **`sub`** when present (normalized); otherwise **`trogon-mcp-tenant`** header when legacy path is allowed; otherwise anonymous.

### JSON-RPC codes (gateway)

Client-visible errors partially align with `MCP_GATEWAY_PLAN.md` Trogon allocation (`rpc_codes`): `-32100` policy deny after SpiceDB refusal, `-32102` upstream timeout, `-32103` upstream request failure, `-32106` expired JWT bearer, `-32107` SpiceDB unreachable / PDP error, `-32109` auth required when **`require`** and gate applies, `-32110` malformed or invalid Bearer token under **`require`**.

| Variable | Meaning |
|---------|---------|
| `MCP_GATEWAY_SPICEDB_ENDPOINT` | SpiceDB gRPC endpoint; empty/unset ⇒ no SpiceDB, allow-all |
| `MCP_GATEWAY_SPICEDB_TOKEN` | Optional Bearer preshared key |
| `MCP_GATEWAY_SPICEDB_INSECURE` | Truthy ⇒ plaintext HTTP/2 (local `serve`) |
| `MCP_GATEWAY_SPICEDB_TOOL_OBJECT_TYPE` | Resource definition for `tools/call` |
| `MCP_GATEWAY_SPICEDB_RESOURCE_OBJECT_TYPE` | Resource definition for `resources/read` |
| `MCP_GATEWAY_SPICEDB_SUBJECT_OBJECT_TYPE` | Subject definition for callers |
| `MCP_GATEWAY_SPICEDB_PERMISSION` | Permission checked on **`trogon/mcp_tool`** (`call`) |
| `MCP_GATEWAY_SPICEDB_READ_PERMISSION` | Permission checked on **`trogon/mcp_resource`** (`read`) |
| `MCP_GATEWAY_SPICEDB_ANONYMOUS_SUBJECT_ID` | Substitute principal when `trogon-mcp-tenant` is absent |

Example schema snippet (adapt relations to your model):

```
definition trogon/principal {}

definition trogon/mcp_tool {
  relation caller: trogon/principal
  permission call = caller
}

definition trogon/mcp_resource {
  relation reader: trogon/principal
  permission read = reader
}
```

## Integration checks

Skipped by default (`#[ignore]`). With a reachable broker:

```bash
export NATS_URL=nats://127.0.0.1:4222
cargo test -p trogon-mcp-gateway -- --ignored
```
