# A2A runtime environment reference

Environment variables and CLI flags for A2A binaries and embedders. Shared timeout and prefix knobs live in the **`a2a_nats`** crate (`rsworkspace/crates/a2a-nats`).

## Shared `a2a_nats` configuration

Embedders build an [`a2a_nats::Config`](../../../rsworkspace/crates/a2a-nats/src/config.rs) (prefix + `trogon_nats::NatsConfig`), then apply runtime overrides:

| Function | Purpose |
|----------|---------|
| [`apply_timeout_overrides`](../../../rsworkspace/crates/a2a-nats/src/config.rs) | Reads env and overrides operation timeout, task timeout, max concurrent client tasks, and push DLQ caller segment on an existing `Config`. |
| [`nats_connect_timeout`](../../../rsworkspace/crates/a2a-nats/src/config.rs) | Returns NATS dial timeout from env (independent of `Config`). |

### Exported env constant names (`a2a_nats`)

Rust identifiers map to these **process environment variable names** (string values):

| Rust constant | Env var string | Default (when unset) |
|---------------|----------------|----------------------|
| `ENV_A2A_PREFIX` | `A2A_PREFIX` | `a2a` |
| `ENV_OPERATION_TIMEOUT_SECS` | `A2A_OPERATION_TIMEOUT_SECS` | 30 s (unary JSON-RPC) |
| `ENV_TASK_TIMEOUT_SECS` | `A2A_TASK_TIMEOUT_SECS` | 7200 s (streaming task completion) |
| `ENV_CONNECT_TIMEOUT_SECS` | `A2A_CONNECT_TIMEOUT_SECS` | 10 s |
| `ENV_PUSH_DLQ_CALLER_SEGMENT` | `A2A_PUSH_DLQ_CALLER_SEGMENT` | `_` |
| `ENV_MAX_CONCURRENT_CLIENT_TASKS` | `A2A_MAX_CONCURRENT_CLIENT_TASKS` | `256` (`0` normalizes to `1`) |

Constants are defined in [`constants.rs`](../../../rsworkspace/crates/a2a-nats/src/constants.rs). The crate root re-exports `ENV_A2A_PREFIX`, `ENV_MAX_CONCURRENT_CLIENT_TASKS`, and `ENV_PUSH_DLQ_CALLER_SEGMENT`; the timeout env names are used internally by `apply_timeout_overrides` and `nats_connect_timeout`.

Timeout overrides require integer values ≥ 1; invalid or sub-minimum values log a warning and keep the prior/default value.

### Shared NATS connection env (`trogon_nats`)

Binaries that call `NatsConfig::from_env` honor **`trogon_nats`** auth and URL variables (priority order for auth):

| Variable | Required | Default | Meaning |
|----------|----------|---------|---------|
| `NATS_URL` | no | `localhost:4222` | Comma-separated server list |
| `NATS_CREDS` | no | — | Credentials file path (highest auth priority) |
| `NATS_NKEY` | no | — | NKey seed |
| `NATS_USER` / `NATS_PASSWORD` | no | — | User/password pair |
| `NATS_TOKEN` | no | — | Token auth |

---

## Per-binary reference

### `a2a-nats-agent`

NATS agent bridge: subscribes on `{prefix}.agent.{agent_id}.*`, provisions JetStream streams on startup (`provision_streams`).

| Variable | Required | Default | Meaning |
|----------|----------|---------|---------|
| `A2A_AGENT_ID` | **yes** | — | Agent identity segment in NATS subjects |
| `A2A_PREFIX` | no | `a2a` | Subject prefix (`ENV_A2A_PREFIX`) |
| `NATS_URL` | no | `localhost:4222` | NATS servers (`NatsConfig::from_env`) |
| `NATS_CREDS` / `NATS_NKEY` / `NATS_USER` / `NATS_PASSWORD` / `NATS_TOKEN` | no | — | NATS auth (see above) |
| `A2A_OPERATION_TIMEOUT_SECS` | no | 30 | Via `apply_timeout_overrides` |
| `A2A_TASK_TIMEOUT_SECS` | no | 7200 | Via `apply_timeout_overrides` |
| `A2A_MAX_CONCURRENT_CLIENT_TASKS` | no | 256 | Via `apply_timeout_overrides` |
| `A2A_PUSH_DLQ_CALLER_SEGMENT` | no | `_` | Via `apply_timeout_overrides` |
| `A2A_PUSH_DLQ_DEDUP_LRU_SIZE` | no | `1024` | In-process push DLQ dedup LRU on agent `Bridge` |
| `A2A_PUSH_DLQ_DEDUP_WINDOW_SECS` | no | `120` | JetStream `duplicate_window` when provisioning **`A2A_PUSH_DLQ`** |
| `A2A_CONNECT_TIMEOUT_SECS` | no | 10 | Via `nats_connect_timeout` |
| `A2A_EVENTS_MAX_AGE_SECS` | no | 86400 (24h) | Per-Account **`A2A_EVENTS`** JetStream `max_age` at provision time |

Source: [`a2a-nats-agent/src/runtime.rs`](../../../rsworkspace/crates/a2a-nats-agent/src/runtime.rs).

---

### `a2a-nats-server`

HTTP JSON-RPC (and SSE streaming) front-end over `a2a_nats::Client`. Full run instructions and route notes: [`a2a-nats-server/README.md`](../../../rsworkspace/crates/a2a-nats-server/README.md).

| Variable | Required | Default | Meaning |
|----------|----------|---------|---------|
| `A2A_AGENT_ID` | **yes** | — | Target agent for outbound NATS RPC |
| `A2A_PREFIX` | no | `a2a` | Subject prefix |
| `A2A_HTTP_BIND` | no | `0.0.0.0:8080` | TCP listen address (`ENV_HTTP_BIND` in server runtime) |
| `A2A_USE_GATEWAY` | no | off | Truthy (`1`, `true`, `yes`, `on`) routes unary traffic via `{prefix}.gateway.{agent_id}.*` for `a2a-gateway` |
| `A2A_GATEWAY_CALLER_JWT` | when gateway on | — | Auth-callout-minted User JWT attached as [`A2a-Caller-Jwt`](../../../rsworkspace/crates/a2a-auth-callout/src/caller_jwt_header.rs) on every `{prefix}.gateway.*` publish (typically the same JWT as the NATS connection credential) |
| `NATS_URL` | no | `localhost:4222` | NATS servers |
| `NATS_CREDS` / `NATS_NKEY` / `NATS_USER` / `NATS_PASSWORD` / `NATS_TOKEN` | no | — | NATS auth |
| `A2A_OPERATION_TIMEOUT_SECS` | no | 30 | Via `apply_timeout_overrides` |
| `A2A_TASK_TIMEOUT_SECS` | no | 7200 | Via `apply_timeout_overrides` |
| `A2A_MAX_CONCURRENT_CLIENT_TASKS` | no | 256 | Via `apply_timeout_overrides` |
| `A2A_PUSH_DLQ_CALLER_SEGMENT` | no | `_` | Via `apply_timeout_overrides` |
| `A2A_PUSH_DLQ_DEDUP_LRU_SIZE` | no | `1024` | In-process push DLQ dedup LRU on agent `Bridge` |
| `A2A_PUSH_DLQ_DEDUP_WINDOW_SECS` | no | `120` | JetStream `duplicate_window` when provisioning **`A2A_PUSH_DLQ`** |
| `A2A_CONNECT_TIMEOUT_SECS` | no | 10 | Via `nats_connect_timeout` |

Source: [`a2a-nats-server/src/runtime.rs`](../../../rsworkspace/crates/a2a-nats-server/src/runtime.rs).

---

### `a2a-nats-stdio`

Line-delimited JSON-RPC over stdin/stdout using `a2a_nats::Client` (no HTTP).

| Variable | Required | Default | Meaning |
|----------|----------|---------|---------|
| `A2A_AGENT_ID` | **yes** | — | Target agent for outbound NATS RPC |
| `A2A_PREFIX` | no | `a2a` | Subject prefix |
| `NATS_URL` | no | `localhost:4222` | NATS servers |
| `NATS_CREDS` / `NATS_NKEY` / `NATS_USER` / `NATS_PASSWORD` / `NATS_TOKEN` | no | — | NATS auth |
| `A2A_OPERATION_TIMEOUT_SECS` | no | 30 | Via `apply_timeout_overrides` |
| `A2A_TASK_TIMEOUT_SECS` | no | 7200 | Via `apply_timeout_overrides` |
| `A2A_MAX_CONCURRENT_CLIENT_TASKS` | no | 256 | Via `apply_timeout_overrides` |
| `A2A_PUSH_DLQ_CALLER_SEGMENT` | no | `_` | Via `apply_timeout_overrides` |
| `A2A_PUSH_DLQ_DEDUP_LRU_SIZE` | no | `1024` | In-process push DLQ dedup LRU on agent `Bridge` |
| `A2A_PUSH_DLQ_DEDUP_WINDOW_SECS` | no | `120` | JetStream `duplicate_window` when provisioning **`A2A_PUSH_DLQ`** |
| `A2A_CONNECT_TIMEOUT_SECS` | no | 10 | Via `nats_connect_timeout` |

Source: [`a2a-nats-stdio/src/runtime.rs`](../../../rsworkspace/crates/a2a-nats-stdio/src/runtime.rs).

---

### `a2a-nats-discovery`

Agent catalog discovery and registrar over NATS KV. Configuration is via **clap `Args`** (env-backed long flags).

| Flag / env | Required | Default | Meaning |
|------------|----------|---------|---------|
| `--nats-url` / `NATS_URL` | no | `localhost:4222` | Comma-separated NATS URLs (overrides `NATS_URL` in `NatsConfig` when set on CLI) |
| `--prefix` / `A2A_PREFIX` | no | `a2a` | Subject prefix for `{prefix}.discover.*` |
| `NATS_CREDS` / `NATS_NKEY` / `NATS_USER` / `NATS_PASSWORD` / `NATS_TOKEN` | no | — | NATS auth via `NatsConfig::from_env` |
| `A2A_CONNECT_TIMEOUT_SECS` | no | 10 | Via `nats_connect_timeout` (not exposed as CLI flag) |
| `A2A_SPICEDB_ENDPOINT` | no | — | Authzed/SpiceDB gRPC endpoint for federated import gate (`SpiceDbImportGate`); unset ⇒ deny-only |
| `A2A_SPICEDB_TOKEN` | no | — | Bearer token for Authzed API; required when `A2A_SPICEDB_ENDPOINT` is set |
| `A2A_SPICEDB_ZEDTOKEN_TTL_SECS` | no | 30 | ZedToken cache TTL for import-gate bulk checks |
| `A2A_DISCOVERY_OPERATOR_KEYS` | no | — | Comma-separated operator Ed25519 registry (`key_id:hexpubkey,...`) for federated discover export signatures; unset ⇒ lab `AllowAllOperatorSignatureGate` |
| `A2A_DISCOVERY_SIGNATURE_MAX_AGE_SECS` | no | 604800 (7 days) | Maximum age for operator-signed discover export envelopes |

Source: [`a2a-nats-discovery/src/config.rs`](../../../rsworkspace/crates/a2a-nats-discovery/src/config.rs), [`runtime.rs`](../../../rsworkspace/crates/a2a-nats-discovery/src/runtime.rs), [`signed_export/`](../../../rsworkspace/crates/a2a-nats-discovery/src/signed_export), [`operator_signature_gate.rs`](../../../rsworkspace/crates/a2a-nats-discovery/src/operator_signature_gate.rs), [`a2a-nats/src/catalog/import_gate/spicedb/`](../../../rsworkspace/crates/a2a-nats/src/catalog/import_gate/spicedb).

---

### `a2a-gateway`

Subscribes on `{prefix}.gateway.>` and forwards ingress to mapped `{prefix}.agent.{agent_id}.*` subjects.

| Flag / env | Required | Default | Meaning |
|------------|----------|---------|---------|
| `--nats-url` / `NATS_URL` | no | `localhost:4222` | Comma-separated NATS URLs |
| `--prefix` / `A2A_PREFIX` | no | `a2a` | A2A subject prefix |
| `--queue-group` / `A2A_GATEWAY_QUEUE_GROUP` | no | — | Optional NATS queue group for gateway subscribers; unset ⇒ ephemeral subscriber |
| `NATS_CREDS` / `NATS_NKEY` / `NATS_USER` / `NATS_PASSWORD` / `NATS_TOKEN` | no | — | NATS auth via `NatsConfig::from_env` |
| `A2A_CONNECT_TIMEOUT_SECS` | no | 10 | Via `nats_connect_timeout` |
| `A2A_GATEWAY_POLICY_BUNDLE_DIR` | no | — | Enables Wasmtime-hosted substrate Tier-3 redaction + Tier-2 policy seam on ingress; also the root for `<skill_id>.skill.toml` manifests (see [Tier-3 skills catalog](policy/tier3-skills-catalog.md)) |
| `A2A_GATEWAY_TIER2_CEL_ENABLED` | no | off | Truthy loads `{bundle_dir}/tier2/*.cel` and runs the CEL evaluator; off keeps noop Tier-2 (see [Tier-2 CEL](policy/tier2-cel.md)) |
| `A2A_GATEWAY_TIER3_REDACTION_ENABLED` | no | off | Truthy runs authoritative Tier-3 WASM redaction on ingress after Tier-2 (see [Tier-3 redaction](policy/tier3-redaction.md)); requires `A2A_GATEWAY_POLICY_BUNDLE_DIR` + preloaded skills |
| `A2A_GATEWAY_POLICY_SKILLS` | no | — | Comma-separated skill slugs; preload `{skill}.wasm` and `{skill}.manifest.json` from `A2A_GATEWAY_POLICY_BUNDLE_DIR` (missing files skipped) |
| `A2A_GATEWAY_TIER3_SIGNING_PUBKEY` | no | — | Hex-encoded 32-byte ed25519 public key (no `0x` prefix); when set, every preloaded skill must ship a valid `{skill}.sig` envelope verified at preload (see [Tier-3 redaction](policy/tier3-redaction.md)) |
| `A2A_GATEWAY_UNARY_DEADLINE_SECS` | no | inherits [`DEFAULT_OPERATION_TIMEOUT`](../../../rsworkspace/crates/a2a-nats/src/constants.rs) | Applies to `message.send` unary forwards |
| `A2A_GATEWAY_AUDIT_PUBLISH` | no | off | Truthy publishes gateway ingress [`AuditEnvelope`](../../../rsworkspace/crates/a2a-nats/src/audit/envelope.rs) JSON |
| `A2A_GATEWAY_PUSH_DLQ_MIRROR` | no | off | When **`on`**, runs a JetStream pull consumer that mirrors agent push DLQ envelopes to **`{prefix}.push.dlq.mirror.{caller_id}.{task_id}`** (see [push DLQ ops](../how-to/operators/push-dlq-triage.md)) |
| `A2A_GATEWAY_PUSH_DLQ_DURABLE` | no | `a2a-gateway-push-dlq-mirror` | Durable name for the gateway push DLQ mirror consumer when mirror mode is enabled |
| `A2A_PUSH_DLQ_DEDUP_LRU_SIZE` | no | `1024` | In-process LRU capacity for suppressing duplicate push DLQ publishes (agent `Bridge` + gateway mirror) |
| `A2A_PUSH_DLQ_DEDUP_WINDOW_SECS` | no | `120` | JetStream `duplicate_window` on **`A2A_PUSH_DLQ`** stream provisioning |
| `A2A_GATEWAY_EVENTS_PULL` | no | off | Truthy spawns durable JetStream pull consumer on **`A2A_EVENTS`** (`{prefix}.task.*.events.*`); forwards to **`{prefix}.gateway.egress.{req_id}`** |
| `A2A_GATEWAY_EVENTS_MAX_ACK_PENDING` | no | `1024` | JetStream **`max_ack_pending`** for the gateway events consumer |
| `A2A_GATEWAY_EVENTS_FETCH_BATCH` | no | `1` | Pull fetch batch size (flow-control boundary) |
| `A2A_GATEWAY_EVENTS_FETCH_HEARTBEAT_SECS` | no | `5` | Pull fetch heartbeat interval |
| `A2A_GATEWAY_EVENTS_MAX_INFLIGHT_PER_CALLER` | no | `32` | Max concurrent in-flight forwards per **`req_id`** (caller fan-out cap) |
| `A2A_GATEWAY_TIER1_SPICEDB_ENABLED` | no | off | Truthy (`on`) enables Tier-1 SpiceDB `BulkCheckPermission` on ingress before Tier-2 |
| `A2A_GATEWAY_TIER1_SPICEDB_ENDPOINT` | when Tier-1 on | — | Authzed/SpiceDB gRPC endpoint for gateway Tier-1 gate |
| `A2A_GATEWAY_TIER1_SPICEDB_TOKEN` | when Tier-1 on | — | Bearer token for gateway Tier-1 Authzed client |
| `A2A_GATEWAY_TIER1_ZEDTOKEN_TTL_SECS` | no | `60` | Session ZedToken cache TTL for Tier-1 bulk checks (keyed by JWT `sub` + Account) |
| `A2A_GATEWAY_TIER1_DECLARATIVE_ENABLED` | no | off | Truthy loads `*.tier1.toml` declarative allow/deny matrices from `A2A_GATEWAY_TIER1_BUNDLE_DIR`; runs after SpiceDB Tier-1 (see [Tier-1 declarative](policy/tier1-declarative.md)) |
| `A2A_GATEWAY_TIER1_BUNDLE_DIR` | when declarative on | — | Directory of `*.tier1.toml` bundle files for Tier-1 declarative policy |
| `A2A_GATEWAY_JWT_AUDIENCE` | no | `A2A_PREFIX` | Expected `aud` on minted User JWTs in [`CALLER_JWT_HEADER_NAME`](../../../rsworkspace/crates/a2a-auth-callout/src/caller_jwt_header.rs) (`A2a-Caller-Jwt`) |
| `AUTH_CALLOUT_SIGNING_KEY_SOURCE` | no | `env` | Gateway JWT verification uses the same signing-key custody as `a2a-auth-callout` (`env` \| `file`; see auth-callout section) |
| `A2A_GATEWAY_TRUST_CALLER_HEADERS` | no | off | **Deprecated.** Labs-only fallback: honor [`GATEWAY_PRINCIPAL_HEADER`](../../../rsworkspace/crates/a2a-nats/src/constants.rs) / [`GATEWAY_CALLER_ID_HEADER`](../../../rsworkspace/crates/a2a-nats/src/constants.rs) only when no verified `A2a-Caller-Jwt` is present. Scheduled for removal once all publishers attach the JWT header. |

Caller attribution: publishers attach the auth-callout-minted User JWT in **`A2a-Caller-Jwt`** on every publish to `{prefix}.gateway.>`. The gateway verifies signature, expiry, and audience via `JwtHeaderCallerIdentitySource`. `a2a-bridge` sets this header from the per-request mint (same JWT used for the NATS connection token). In-tree **`a2a-nats::Client`** carries a `MintedUserJwt` when [`routing_via_gateway_ingress`](../../../rsworkspace/crates/a2a-nats/src/client/handle.rs) is enabled (`a2a-nats-server` supplies it via `A2A_GATEWAY_CALLER_JWT` when `A2A_USE_GATEWAY` is on). External NATS-native publishers that bypass `a2a-nats::Client` must attach the header themselves; until they do, `A2A_GATEWAY_TRUST_CALLER_HEADERS` remains the labs-only fallback.

CLI/env wiring: [`a2a-gateway/src/config.rs`](../../../rsworkspace/crates/a2a-gateway/src/config.rs). Runtime: [`a2a-gateway/src/runtime.rs`](../../../rsworkspace/crates/a2a-gateway/src/runtime.rs).

---

### `a2a-auth-callout`

NATS auth callout subscriber on `$SYS.REQ.USER.AUTH`; mints short-lived User JWTs for tenant Accounts.

| Variable | Required | Default | Meaning |
|----------|----------|---------|---------|
| `NATS_URL` | no | `nats://localhost:4222` | NATS servers |
| `AUTH_CALLOUT_SERVER_NKEY_PUBLIC` | **yes** | — | Public NKey of `authorization.auth_callout.issuer` — verify server-signed authorization **request** JWTs |
| `AUTH_CALLOUT_ISSUER_NKEY_SEED` | **yes** | — | Seed for the callout account signing NKey — sign authorization **response** JWTs |
| `AUTH_CALLOUT_XKEY_SEED` | no | — | Account XKey seed when `auth_callout.xkey` is set on the server (decrypt requests / encrypt responses) |
| `AUTH_CALLOUT_SERVER_XKEY_PUBLIC` | when encryption enabled | — | Server **persistent** XKey public key (`nats-server` seals requests with this keypair) |
| `AUTH_CALLOUT_ALLOWED_ACCOUNTS` | **yes** | — | Comma-separated tenant Account names the resolver may bind |
| `AUTH_CALLOUT_SIGNING_KEY_SOURCE` | no | `env` | `env` \| `file` \| `vault` (vault not implemented) |
| `AUTH_CALLOUT_SIGNING_SECRET` | env source | `dev-secret-not-for-production` if unset | Current HS256 secret (dev-only custody) |
| `AUTH_CALLOUT_SIGNING_SECRET_PREVIOUS` | no | — | Previous secret for rotation overlap (env source) |
| `AUTH_CALLOUT_SIGNING_KEY_PATH` | file source | — | Path to current signing key bytes |
| `AUTH_CALLOUT_SIGNING_KEY_PREVIOUS_PATH` | no | — | Path to previous signing key bytes (rotation overlap) |
| `AUTH_CALLOUT_USER_JWT_TTL_SECS` | no | `300` | Minted User JWT lifetime |
| `AUTH_CALLOUT_OIDC_ISSUER` | no | — | OIDC issuer URL; omit to disable OIDC verifier |
| `AUTH_CALLOUT_OIDC_AUDIENCES` | when OIDC on | — | Comma-separated allowed `aud` values |
| `AUTH_CALLOUT_MTLS_TRUST_ANCHORS` | no | — | PEM bundle path for mTLS client cert verification |

Deployment notes: [`A2A_AUTH_CALLOUT_DEPLOYMENT.md`](../how-to/operators/auth-callout-deployment.md).

Source: [`a2a-auth-callout/src/main.rs`](../../../rsworkspace/crates/a2a-auth-callout/src/main.rs).

---

### `a2a-bridge`

HTTPS shim that exchanges NATS-signed JWT mints for gateway unary + SSE task streams (`AsyncNatsToken*` clients).

| Variable | Required | Default | Meaning |
|----------|----------|---------|---------|
| `A2A_BRIDGE_TRANSPORT` | no | `stub` | **`nats`** enables anonymous bootstrap `ConnectOptions::connect(NATS_URL)` + `AuthCalloutJsonMintClient<AsyncNatsAuthMintWire>` + publisher/JetStream ports |
| `NATS_URL` | NATS transport | `nats://127.0.0.1:4222` | Servers for bootstrap + bearer connections |
| `BRIDGE_LISTEN_ADDR` | no | `127.0.0.1:7443` | HTTPS listen address |
| `BRIDGE_CONNECT_TIMEOUT_SECS` | no | `30` | Dial budget when transport is `nats` |
| `BRIDGE_AUTH_MINT_TIMEOUT_SECS` | no | `30` | Budget for mint replies on `AUTH_CALLOUT_MINT_SUBJECT` |
| `BRIDGE_GATEWAY_RPC_TIMEOUT_SECS` | no | `180` | Unary RPC + SSE consumer wiring budget |
| `AUTH_CALLOUT_MINT_SUBJECT` | no | `a2a.bridge.auth.callout.request` | Override JSON-RPC mint NATS subject (`AuthCalloutJsonMintClient::<AsyncNatsAuthMintWire>::default_mint_subject()`) |

Source: [`a2a-bridge/src/main.rs`](../../../rsworkspace/crates/a2a-bridge/src/main.rs), [`inbound.rs`](../../../rsworkspace/crates/a2a-bridge/src/inbound.rs).

---

## Related docs

- [Documentation index](../../A2A_DOCS_INDEX.md)
- [Push DLQ operations](../how-to/operators/push-dlq-triage.md)
- [NSC account bootstrap](../how-to/operators/nsc-account-bootstrap.md)
- [Architecture plan](../explanation/architecture.md)
- [Open work tracker](../explanation/architecture.md)
