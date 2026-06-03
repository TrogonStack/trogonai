# Environment variable reference

Catalog of every runtime env var consumed by the v0.1 binaries. Defaults
are listed where the code falls back to one. Per-crate READMEs are the
final source of truth — this page consolidates them for operators.

## `trogon-mcp-gateway`

| Var | Default | Notes |
|---|---|---|
| `MCP_GATEWAY_MCP_PREFIX` | `mcp` | Subject prefix on the data plane. |
| `MCP_GATEWAY_PROBE_LISTEN_ADDR` | _unset → no probe listener_ | `host:port` for `/healthz` + `/readyz`. |
| `MCP_GATEWAY_QUEUE_GROUP` | `mcp-gateway` | NATS queue group for ingress. |
| `MCP_GATEWAY_AUDIT_STREAM` | `MCP_AUDIT` | JetStream stream for audit envelopes. |
| `MCP_GATEWAY_AUDIT_STREAM_NAME` | _alias of above_ | Legacy name kept for compatibility. |
| `MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT` | `false` | Skip JetStream init on boot (when operator pre-provisions). |
| `MCP_GATEWAY_JWT_MODE` | `validate` | `disabled`, `audience-shadow`, `validate`, `mTLS`. |
| `MCP_GATEWAY_JWT_ISSUERS` | _empty_ | Comma-separated trusted issuers. |
| `MCP_GATEWAY_JWT_JWKS_URI` | _empty_ | Override JWKS URI. |
| `MCP_GATEWAY_JWT_RSA_PUBLIC_KEY_PEM` | _empty_ | Inline PEM (dev). |
| `MCP_GATEWAY_JWT_AUDIENCE` | _empty_ | Required `aud` claim. |
| `MCP_GATEWAY_JWT_TENANT_CLAIM` | `trogon-mcp-tenant` | Header / claim carrying tenant id. |
| `MCP_GATEWAY_JWT_BEARER_HEADER` | `authorization` | Header carrying the JWT. |
| `MCP_GATEWAY_JWT_LEEWAY_SECS` | `0` | Clock-skew tolerance. |
| `MCP_GATEWAY_SPICEDB_ENDPOINT` | _empty → allow-all_ | gRPC endpoint. |
| `MCP_GATEWAY_SPICEDB_TOKEN` | _empty_ | Preshared key (token mode). |
| `MCP_GATEWAY_SPICEDB_INSECURE` | `false` | Plaintext HTTP/2 (`spicedb serve`). |
| `MCP_GATEWAY_SPICEDB_TOOL_OBJECT_TYPE` | `trogon/mcp_tool` | Resource definition for `tools/call`. |
| `MCP_GATEWAY_SPICEDB_RESOURCE_OBJECT_TYPE` | `trogon/mcp_resource` | Resource definition for `resources/read`. |
| `MCP_GATEWAY_SPICEDB_SUBJECT_OBJECT_TYPE` | `trogon/principal` | Subject type for callers. |
| `MCP_GATEWAY_SPICEDB_PERMISSION` | `call` | Permission on `mcp_tool`. |
| `MCP_GATEWAY_SPICEDB_READ_PERMISSION` | `read` | Permission on `mcp_resource`. |
| `MCP_GATEWAY_SPICEDB_ANONYMOUS_SUBJECT_ID` | _empty_ | Substitute principal when tenant header is absent. |
| `MCP_GATEWAY_SPICEDB_ZEDTOKEN_CACHE_TTL_SECS` | `0` | Per-session ZedToken cache TTL. |
| `MCP_GATEWAY_SPICEDB_ZEDTOKEN_CACHE_TTL_MAX_SECS` | `0` | Hard cap. |
| `MCP_GATEWAY_HIERARCHICAL_POLICY_ENFORCE` | `false` | Enable hierarchical overlay merge. |
| `MCP_GATEWAY_CHAIN_RESOLUTION_MODE` | _empty_ | Virtual MCP chain resolution mode. |
| `MCP_GATEWAY_TRUSTED_MINT_ISSUERS` | _empty_ | Issuers allowed to mint via STS. |
| `MCP_GATEWAY_MESH_TOKEN_TTL_SECS` | `300` | TTL for STS-minted mesh tokens. |
| `MCP_GATEWAY_MESH_CACHE_MAX_ENTRIES` | `1024` | Mesh-token cache size. |
| `MCP_GATEWAY_REGISTRY_SUBJECT` | _empty_ | Registry subscription subject. |
| `MCP_GATEWAY_IDENTITY_SUB` | _empty_ | Gateway's own JWT `sub`. |
| `MCP_GATEWAY_IDENTITY_AGENT_ID` | _empty_ | Gateway's own agent identifier. |
| `MCP_GATEWAY_IDENTITY_WKL` | _empty_ | Workload identity (SPIFFE). |
| `MCP_GATEWAY_AUDIENCE` | _empty_ | Audience the gateway claims for upstream calls. |
| `MCP_GATEWAY_AGENT_IDENTITY` | _empty_ | Outgoing agent identity for upstream. |
| `MCP_GATEWAY_ACTOR_TOKEN` | _empty_ | Static actor token (dev). |

## `a2a-gateway`

| Var | Default | Notes |
|---|---|---|
| `A2A_GATEWAY_QUEUE_GROUP` | `a2a-gateway` | Ingress queue group. |
| `A2A_GATEWAY_UNARY_DEADLINE_SECS` | `30` | `message/send` deadline; floored to 1s. |
| `A2A_GATEWAY_JWT_AUDIENCE` | _empty_ | Required `aud`. |
| `A2A_GATEWAY_TRUST_CALLER_HEADERS` | `false` | Trust ingress headers without JWT (dev only). |
| `A2A_GATEWAY_CALLER_JWT` | _empty_ | Static caller JWT (dev). |
| `A2A_GATEWAY_AUDIT_PUBLISH` | `true` | Toggle gateway audit envelopes. |
| `A2A_GATEWAY_TIER1_DECLARATIVE_ENABLED` | `true` | Tier-1 declarative policy. |
| `A2A_GATEWAY_TIER1_BUNDLE_DIR` | _empty_ | Bundle dir (filesystem). |
| `A2A_GATEWAY_TIER1_SPICEDB_ENABLED` | `false` | Tier-1 SpiceDB check. |
| `A2A_GATEWAY_TIER1_SPICEDB_ENDPOINT` | _empty_ | gRPC endpoint. |
| `A2A_GATEWAY_TIER1_SPICEDB_TOKEN` | _empty_ | Preshared key. |
| `A2A_GATEWAY_TIER1_ZEDTOKEN_TTL_SECS` | `0` | Cache TTL. |
| `A2A_GATEWAY_TIER2_CEL_ENABLED` | `false` | Tier-2 CEL (deferred — see ADR 0012). |
| `A2A_GATEWAY_TIER3_REDACTION_ENABLED` | `false` | Tier-3 redaction. |
| `A2A_GATEWAY_TIER3_SIGNING_PUBKEY` | _empty_ | Required when Tier-3 bundles are signed. |
| `A2A_GATEWAY_POLICY_BUNDLE_DIR` | _empty_ | Bundle dir for all tiers. |
| `A2A_GATEWAY_POLICY_SKILLS` | _empty_ | Skills allowlist. |
| `A2A_GATEWAY_STREAMING_INGRESS` | `true` | Enable `message/stream` ingress. |
| `A2A_GATEWAY_STREAMING_MAX_ACK_PENDING` | `64` | JetStream consumer ack window. |
| `A2A_GATEWAY_STREAMING_MAX_INFLIGHT` | `16` | Per-caller inflight cap. |
| `A2A_GATEWAY_EVENTS` | `A2A_EVENTS` | Events stream name. |
| `A2A_GATEWAY_EVENTS_PULL` | `false` | Pull-consumer mode. |
| `A2A_GATEWAY_EVENTS_FETCH_BATCH` | `8` | Pull batch size. |
| `A2A_GATEWAY_EVENTS_FETCH_HEARTBEAT_SECS` | `5` | Pull heartbeat. |
| `A2A_GATEWAY_EVENTS_MAX_ACK_PENDING` | `64` | Events ack window. |
| `A2A_GATEWAY_EVENTS_MAX_INFLIGHT_PER_CALLER` | `16` | Per-caller event cap. |
| `A2A_GATEWAY_PUSH_DLQ_MIRROR` | `false` | Mirror push DLQ on the gateway side. |
| `A2A_GATEWAY_PUSH_DLQ_DURABLE` | _empty_ | Mirror durable name. |

## `a2a-bridge`

| Var | Default | Notes |
|---|---|---|
| `A2A_BRIDGE_LISTEN_ADDR` | `0.0.0.0:8443` | HTTPS listener. |
| `A2A_BRIDGE_TRANSPORT` | `stub` | `stub` (v0.1 default) or real `auth-callout` (v0.2). |

## Common

| Var | Default | Notes |
|---|---|---|
| `NATS_URL` | `nats://nats:4222` | NATS connection. |
| `NATS_CREDS` | _empty_ | NSC user credentials (file path or inline). |
| `TROGON_OTEL_ENDPOINT` | _empty_ | OTLP/gRPC endpoint. |
| `TROGON_OTEL_SERVICE_NAME` | per-binary | Override `service.name`. |
| `TROGON_AUDIT_CONSUMER` | _per binary_ | Audit consumer name. |
| `TROGON_GATEWAY_REGIONS` | _empty_ | Comma-separated region list (multi-region routing). |
| `TROGON_SIEM_FORMAT` | _empty_ | `splunk-hec`, `elastic-ecs`. |
| `TROGON_SIEM_SUBJECT` | _empty_ | Subject to publish reshaped envelopes to. |
| `RUST_LOG` | `info` | Tracing filter. |

For the remaining binaries (`trogon-sts`, `trogon-jwks-publisher`,
`a2a-auth-callout`, `a2a-nats-discovery`, `trogon-agent-registry`,
`mcp-nats-server`, `acp-nats-server`, `trogon-traffic-view`), see the
crate README in `rsworkspace/crates/<crate>/README.md`. These crates
read the `NATS_URL` / `TROGON_OTEL_*` common vars plus crate-specific
ones documented inline; v0.2 will consolidate.
