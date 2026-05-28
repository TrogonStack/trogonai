# MCP gateway NATS subject grammar

**Diátaxis:** reference (strict, exhaustive).

**Related:** [overview.md](overview.md), [agent-traffic.md](agent-traffic.md), [integration-touchpoints.md](integration-touchpoints.md), [sts-exchange.md](sts-exchange.md), [registry.md](registry.md).

This page is the single lookup for MCP mesh NATS subjects and KV identifiers used by the gateway and its identity adjacencies. Every subject listed is either cited to source (`path.rs:LINE`) or marked **proposed**. Wire examples use default prefix `mcp`; substitute `{prefix}` when `MCP_PREFIX` is overridden.

---

## 1. Prefix

### 1.1 Configuration

| Item | Value | Source |
|---|---|---|
| Environment variable | `MCP_PREFIX` | `mcp_nats::ENV_MCP_PREFIX` — `rsworkspace/crates/mcp-nats/src/constants.rs:4` |
| Default | `mcp` | `mcp_nats::DEFAULT_MCP_PREFIX` — `constants.rs:3` |
| Loader | `mcp_nats::Config::from_env` | `rsworkspace/crates/mcp-nats/src/config.rs:31-38` |
| Type | `McpPrefix` (`DottedNatsToken`) | `rsworkspace/crates/mcp-nats/src/mcp_prefix.rs:22-33` |

### 1.2 Validation rules (`McpPrefix`)

- Non-empty; max **128 bytes** (UTF-8).
- Dotted segments allowed (`tenant.mcp` is valid).
- Rejects wildcards (`*`, `>`), whitespace, consecutive dots, leading/trailing dots.

See `mcp_prefix.rs:45-58` (tests) and `trogon_nats::DottedNatsToken` — `rsworkspace/crates/trogon-nats/src/nats_token.rs:68-93`.

### 1.3 Propagation

| Consumer | Uses `{prefix}` from `MCP_PREFIX`? | Notes |
|---|---|---|
| `trogon-mcp-gateway` ingress subscribe | Yes | `{prefix}.gateway.request.>` — `gateway.rs:70` |
| Gateway ingress → egress rewrite | Yes | `subject.rs:15-27` |
| Gateway audit JetStream publish | Yes | `audit.rs:112-114`, stream filter `audit.rs:130` |
| `mcp-nats` server/client transports | Yes | `Config::prefix_str()` on all `{prefix}.server.*` / `{prefix}.client.*` builders |
| Approvals subjects | **No (hardcoded `mcp`)** | `approvals/types.rs:41-45` |
| Anomaly / shadow metrics | **No (hardcoded `mcp`)** | `anomaly.rs:8`, `egress/audience.rs:4` |
| STS exchange + STS audit | **No (hardcoded `mcp`)** | `trogon-sts/src/lib.rs:27`, `trogon-sts/src/audit.rs:53-54` |
| Registry lookup + registry audit | **No (hardcoded `mcp`)** | `trogon-agent-registry/src/consumer.rs:17`, `audit.rs:7-11` |
| Trust-bundle / JWKS KV bucket names | **No (fixed bucket strings)** | `trogon-sts/src/lib.rs:28-30` |

Operators who change `MCP_PREFIX` must align NATS account export/import ACLs and update hardcoded subjects separately until prefix propagation lands on those crates.

### 1.4 Process wiring

Gateway loads prefix via `gateway_cli_config` → `Config::from_env` — `trogon-mcp-gateway/src/config.rs:117-139`, `main.rs` (passes `GatewaySettings.mcp`).

---

## 2. Grammar

Notation: `{prefix}` = configured `McpPrefix`; `{token}` = single NATS token; `{dotted}` = one or more dot-separated tokens; `{method_suffix}` = MCP JSON-RPC method mapped to dot segments (see §3).

### 2.1 Top-level EBNF

```ebnf
(* configurable prefix segment — may itself contain dots *)
prefix           ::= dotted-token

(* ── Gateway ingress / egress (trogon-mcp-gateway, mcp-nats) ── *)
gateway-ingress  ::= prefix ".gateway.request." server-id "." method-suffix
server-egress    ::= prefix ".server." server-id "." method-suffix
client-lane      ::= prefix ".client." client-id "." method-suffix

server-id        ::= nats-token
client-id        ::= nats-token
method-suffix    ::= dotted-token   (* derived from JSON-RPC; see §3.2 *)

(* ── Gateway audit (JetStream) ── *)
gateway-audit    ::= prefix ".audit." audit-outcome "." audit-direction "." method-root
audit-outcome    ::= "allow" | "deny" | "error"
audit-direction  ::= "request" | "response" | "callback"   (* only "request" emitted today *)
method-root      ::= nats-token   (* first JSON-RPC segment; dots → "_" in audit only *)

(* ── STS ── *)
sts-exchange     ::= "mcp.sts.exchange"                     (* hardcoded; not {prefix} *)
sts-audit        ::= "mcp.audit.sts." sts-outcome
sts-outcome      ::= "success" | "deny" | "error" | "rate_limit"

(* ── Agent registry ── *)
registry-lookup  ::= "mcp.registry.agent.lookup"            (* hardcoded *)
registry-audit   ::= "mcp.audit.registry." registry-event
registry-event   ::= lookup-outcome | mutation-event | lifecycle-event
lookup-outcome   ::= "lookup.found" | "lookup.notfound" | "lookup.revoked"
mutation-event   ::= "put" | "delete"
lifecycle-event  ::= "registered" | "bumped" | "deprecated" | "revoked"

(* ── Approvals (core NATS) ── *)
approval-wait    ::= "mcp.approvals." request-id
approval-step-up ::= "mcp.approvals.step-up." request-id
approval-grant   ::= prefix ".approvals." request-id ".grant"   (* **proposed** — not in code *)

(* ── Metrics (core NATS, fire-and-forget) ── *)
metric-anomaly   ::= "mcp.metrics.anomaly.features"
metric-gateway   ::= "mcp.metrics.gateway." metric-name
metric-sts       ::= "mcp.metrics.sts." metric-name

(* ── JWKS sidecar (trogon-jwks-publisher) ── *)
jwks-reqrep      ::= "mcp.jwks.mesh.get"

(* ── Wildcard subscription patterns (mcp-nats) ── *)
sub-all-servers  ::= prefix ".server.>"
sub-one-server   ::= prefix ".server." server-id ".>"
sub-all-clients  ::= prefix ".client.>"
sub-one-client   ::= prefix ".client." client-id ".>"

(* ── KV buckets (JetStream KV — not NATS subjects) ── *)
kv-trust         ::= bucket "mcp-trust-bundles" key trust-domain
kv-jwks          ::= bucket "mcp-jwks" key jwks-key
kv-registry      ::= bucket "mcp-agent-registry" key registry-key
```

### 2.2 Gateway ingress

**Pattern:** `{prefix}.gateway.request.{server_id}.{method_suffix}`

| Field | Source |
|---|---|
| Subscribe (gateway worker) | `{prefix}.gateway.request.>` — `gateway.rs:70-72` |
| Rewrite to egress | `{prefix}.gateway.request.{rest}` → `{prefix}.server.{rest}` — `subject.rs:14-27` |
| Minimum segments after `gateway.request.` | `server_id` + at least one method segment — `subject.rs:21-24` |

**Example:** `mcp.gateway.request.filesystem.tools.call` → `mcp.server.filesystem.tools.call` (`subject.rs:35-39`).

### 2.3 Server egress lane

**Pattern:** `{prefix}.server.{server_id}.{method_suffix}`

Built by `mcp-nats` subject types, e.g. `{prefix}.server.{server_id}.tools.list` — `mcp-nats/src/nats/subjects/server/list_tools.rs:18-23`.

#### 2.3.1 Server request methods (`method_suffix`)

| JSON-RPC method | Subject suffix | Source |
|---|---|---|
| `initialize` | `initialize` | `mcp-nats/src/nats/parsing.rs:27` |
| `ping` | `ping` | `parsing.rs:28` |
| `completion/complete` | `completion.complete` | `parsing.rs:29` |
| `logging/setLevel` | `logging.set_level` | `parsing.rs:30` |
| `prompts/list` | `prompts.list` | `parsing.rs:31` |
| `prompts/get` | `prompts.get` | `parsing.rs:32` |
| `resources/list` | `resources.list` | `parsing.rs:33` |
| `resources/templates/list` | `resources.templates.list` | `parsing.rs:34` |
| `resources/read` | `resources.read` | `parsing.rs:35` |
| `resources/subscribe` | `resources.subscribe` | `parsing.rs:36` |
| `resources/unsubscribe` | `resources.unsubscribe` | `parsing.rs:37` |
| `tools/list` | `tools.list` | `parsing.rs:38` |
| `tools/call` | `tools.call` | `parsing.rs:39` |
| `tasks/get` | `tasks.get` | `parsing.rs:40` |
| `tasks/list` | `tasks.list` | `parsing.rs:41` |
| `tasks/result` | `tasks.result` | `parsing.rs:42` |
| `tasks/cancel` | `tasks.cancel` | `parsing.rs:43` |

Full subject list for peer `filesystem`: `mcp-nats/src/nats/subjects/mod.rs:89-109`.

#### 2.3.2 Server → client notifications (published on **client** lane)

Target `{prefix}.client.{client_id}.notifications.*` — `mod.rs:114-148`.

| Notification | Subject suffix |
|---|---|
| Cancelled | `notifications.cancelled` |
| Progress | `notifications.progress` |
| Logging message | `notifications.message` |
| Resource updated | `notifications.resources.updated` |
| Resource list changed | `notifications.resources.list_changed` |
| Tool list changed | `notifications.tools.list_changed` |
| Prompt list changed | `notifications.prompts.list_changed` |
| Elicitation complete | `notifications.elicitation.complete` |

#### 2.3.3 Client → server notifications (published on **server** lane)

Target `{prefix}.server.{server_id}.notifications.*` — `mod.rs:186-189`.

| Notification | Subject suffix |
|---|---|
| Cancelled | `notifications.cancelled` |
| Progress | `notifications.progress` |
| Initialized | `notifications.initialized` |
| Roots list changed | `notifications.roots.list_changed` |

### 2.4 Client lane (bidirectional MCP peer)

**Pattern:** `{prefix}.client.{client_id}.{method_suffix}`

#### 2.4.1 Client request methods

| JSON-RPC method | Subject suffix | Source |
|---|---|---|
| `ping` | `ping` | `parsing.rs:88` |
| `sampling/createMessage` | `sampling.create_message` | `parsing.rs:89` |
| `roots/list` | `roots.list` | `parsing.rs:90` |
| `elicitation/create` | `elicitation.create` | `parsing.rs:91` |

Examples: `mod.rs:152-160`.

### 2.5 Gateway audit subjects

**Wire pattern (implemented):**

```text
{prefix}.audit.{outcome}.{direction}.{method_root}
```

| Component | Values (today) | Source |
|---|---|---|
| `outcome` | `allow`, `deny`, `error` | `gateway.rs:446-449`, `581-591`, `306` |
| `direction` | `request` only | `gateway.rs:456`, `532`, `612` |
| `method_root` | First `/`-segment of JSON-RPC method; `.` → `_` | `audit.rs:116-123` |

**Examples:**

- `mcp.audit.allow.request.tools` — allowed `tools/call` or `tools/list`
- `mcp.audit.deny.request.tools` — SpiceDB / policy deny
- `mcp.audit.error.request.tools` — JWT / chain / backend error

**Not literal today:** `{prefix}.audit.gateway.{decision}.{outcome}` — documentation shorthand in [agent-traffic.md §2.1](agent-traffic.md#21-gateway-tool-traffic) maps to `{outcome}` + `{direction}` + `{method_root}` above.

**Stream capture:** `{prefix}.audit.>` — `audit.rs:130`.

### 2.6 STS subjects

| Subject | Role | Source |
|---|---|---|
| `mcp.sts.exchange` | Request/reply exchange | `trogon-sts/src/lib.rs:27` |
| `mcp.audit.sts.success` | Audit on success | `exchange.rs:94`, `audit.rs:53-54` |
| `mcp.audit.sts.deny` | Audit on policy/token deny | `exchange.rs:111`, `error.rs:79` |
| `mcp.audit.sts.error` | Audit on dependency/server error | `error.rs:74-78` |
| `mcp.audit.sts.rate_limit` | Audit on rate limit (`audit_outcome`) | `error.rs:73` |
| `mcp.audit.sts.latency_violation` | Probe metric publish | `trogon-sts/src/bin/probe.rs:13` |

Gateway STS client override: `MCP_GATEWAY_STS_EXCHANGE_SUBJECT` (default `mcp.sts.exchange`) — see [integration-touchpoints.md §1](integration-touchpoints.md#1-gateway--trogon-sts).

### 2.7 Registry subjects

| Subject | Role | Source |
|---|---|---|
| `mcp.registry.agent.lookup` | Request/reply lookup | `trogon-agent-registry/src/consumer.rs:17` |
| `mcp.audit.registry.lookup.found` | Lookup audit | `trogon-agent-registry/src/audit.rs:7` |
| `mcp.audit.registry.lookup.notfound` | Lookup audit | `audit.rs:8` |
| `mcp.audit.registry.lookup.revoked` | Lookup audit | `audit.rs:9` |
| `mcp.audit.registry.put` | Dev/runtime KV put audit | `audit.rs:10` |
| `mcp.audit.registry.delete` | Dev/runtime KV delete audit | `audit.rs:11` |
| `mcp.audit.registry.registered` | Controller lifecycle | `trogon-agent-registry-controller/src/audit.rs:5` |
| `mcp.audit.registry.bumped` | Controller lifecycle | `audit.rs:6` |
| `mcp.audit.registry.deprecated` | Controller lifecycle | `audit.rs:7` |
| `mcp.audit.registry.revoked` | Controller lifecycle | `audit.rs:8` |

Env override for lookup: `MCP_GATEWAY_REGISTRY_SUBJECT`, `MCP_STS_REGISTRY_SUBJECT` (default `mcp.registry.agent.lookup`).

**proposed:** `mcp.audit.registry.deny` — referenced in [registry.md](registry.md) for invalid manifest sync; not emitted in Rust tree today.

### 2.8 Approvals subjects

| Subject | Role | Source |
|---|---|---|
| `mcp.approvals.{request_id}` | Gateway subscribes; approver publishes decision | `approvals/types.rs:40-41`, `approvals/client.rs:42` |
| `mcp.approvals.step-up.{request_id}` | Step-up flow | `approvals/types.rs:44-45` |

Decision payload: `ApprovalDecisionMessage` JSON (`decision`, `approver`, `expires_at`) — `approvals/types.rs:76-81`. Published on the **same** subject as the waiter (no separate `.grant` suffix).

| Subject | Status |
|---|---|
| `{prefix}.approvals.{request_id}.grant` | **proposed** — not implemented; use `mcp.approvals.{request_id}` today |

Prefix is hardcoded `mcp` in `ApprovalSubject` — not `{prefix}` from `MCP_PREFIX`.

### 2.9 Metrics subjects

| Subject | Publisher | Payload | Source |
|---|---|---|---|
| `mcp.metrics.anomaly.features` | Gateway (best-effort) | `AnomalyFeature` JSON | `anomaly.rs:8`, `11-21` |
| `mcp.metrics.gateway.aud_mismatch_shadow` | Gateway egress shadow | JSON schema `trogon.mcp.metrics.gateway.aud_mismatch_shadow/v1` | `egress/audience.rs:4`, `76` |
| `mcp.metrics.sts.latency` | STS probe binary | JSON schema `trogon.mcp.metrics.sts.latency/v1` | `trogon-sts/src/bin/probe.rs:12`, `87` |
| `mcp.metrics.anomaly.risk` | **proposed** pipeline → gateway read | — | [integration-touchpoints.md §9](integration-touchpoints.md#9-gateway--anomaly-feature-pipeline) |
| `{prefix}.metrics.gateway.{metric_name}` | **proposed** generalization | Only `aud_mismatch_shadow` exists today | — |

All metrics subjects observed in code use hardcoded `mcp` prefix.

### 2.10 JWKS request/reply

| Subject | Queue group | Source |
|---|---|---|
| `mcp.jwks.mesh.get` | `trogon-jwks-publisher` | `trogon-jwks-publisher/src/jwks.rs:8-9` |

### 2.11 KV identifiers (not subjects)

JetStream KV uses **bucket names** and **keys**, not publish subjects. Do not subscribe to `mcp-trust-bundles` as a NATS subject.

| Bucket constant | String | Source |
|---|---|---|
| `TRUST_BUNDLES_KV_BUCKET` | `mcp-trust-bundles` | `trogon-sts/src/lib.rs:30` |
| `JWKS_KV_BUCKET` | `mcp-jwks` | `lib.rs:28`, `trogon-jwks-publisher/src/jwks.rs:6` |
| `BUCKET_NAME` (registry) | `mcp-agent-registry` | `trogon-agent-registry/src/store.rs:14` |

**proposed** (documented in [mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md), not in gateway code): `mcp-gateway-config`, `mcp-sessions`.

---

## 3. Token rules

### 3.1 NATS subject token constraints (platform)

NATS subject tokens:

- Are dot-separated; wildcard tokens `*` (one segment) and `>` (remainder) are reserved for subscriptions.
- Cannot contain spaces; production Trogon code additionally rejects non-ASCII in single tokens.

Trogon enforces stricter validation at construction time so invalid IDs are unrepresentable.

### 3.2 `{prefix}` (`McpPrefix` / `DottedNatsToken`)

| Rule | Detail | Source |
|---|---|---|
| Charset | UTF-8; `.` allowed as separator | `nats_token.rs:68-93` |
| Max length | 128 **bytes** | `nats_token.rs:89-90` |
| Forbidden | `*`, `>`, whitespace, `..`, leading/trailing `.` | `mcp_prefix.rs:51-57`, `nats_token.rs:195-217` |

### 3.3 `{server_id}` / `{client_id}` (`McpPeerId` / `NatsToken`)

| Rule | Detail | Source |
|---|---|---|
| Charset | ASCII only | `nats_token.rs:36-37` |
| Max length | 128 **characters** | `nats_token.rs:33-34` |
| Forbidden | `.`, `*`, `>`, whitespace | `mcp_peer_id.rs:51-54`, `nats_token.rs:36-37` |
| Examples | `filesystem`, `server-1`, `desktop` | `mcp_peer_id.rs:46-47` |

### 3.4 `{method_suffix}` (JSON-RPC → subject mapping)

**Ingress and egress subjects:** JSON-RPC `/` separators become `.` in the subject path. There is **no** `tools/call` → `tools_call` rewrite on the wire.

| JSON-RPC method | Subject tail (after `{server_id}.`) |
|---|---|
| `tools/call` | `tools.call` |
| `resources/read` | `resources.read` |
| `completion/complete` | `completion.complete` |

Mapping table: `mcp-nats/src/nats/parsing.rs:25-44` (server), `85-93` (client).

**Gateway rewrite** preserves the tail verbatim: `{prefix}.gateway.request.{server_id}.{tail}` → `{prefix}.server.{server_id}.{tail}` — `subject.rs:27`.

### 3.5 `{method_root}` (audit segment only)

Used only in `{prefix}.audit.{outcome}.{direction}.{method_root}`:

1. Take JSON-RPC method string (e.g. `tools/call`).
2. Split on `/`; take first non-empty segment → `tools`.
3. Replace any `.` in that segment with `_` (defensive; MCP methods use `/` not `.`).

Implementation: `audit.rs:116-123`.

| JSON-RPC method | `method_root` in audit subject |
|---|---|
| `tools/call` | `tools` → `mcp.audit.allow.request.tools` |
| `resources/read` | `resources` |
| unknown / empty | `unknown` |

This is **audit-only** normalization; it does not change ingress/egress routing subjects.

### 3.6 `{request_id}` (approvals)

Non-empty string; no NATS-token validation at construction — `approvals/types.rs:22-27`. Operators should restrict to URL-safe tokens; wildcards in `request_id` would change subscription semantics.

### 3.7 Registry KV keys (not subject tokens)

| Key pattern | Meaning | Source |
|---|---|---|
| `{agent_id}/@latest` | Pointer to current record | `store.rs:93-95` |
| `{agent_id}/{version}` | Versioned record (**proposed** write path for controller) | [registry.md](registry.md) |
| Trust domain string (e.g. `acme.local`) | Trust bundle PEM | `cache.rs:127-150`, `publish_trust_bundle.rs:44` |
| `mesh/current` | Mesh JWKS blob | `jwks.rs:7`, `trogon-sts/src/lib.rs:29` |

---

## 4. Wildcards

NATS wildcards apply to **subscriptions**, not to published subjects.

| # | Subscriber | Pattern | Wildcard | Purpose | Source |
|---|---|---|---|---|---|
| 1 | MCP gateway workers | `{prefix}.gateway.request.>` | `>` | HA queue-group ingress | `gateway.rs:70-72` |
| 2 | MCP server transport | `{prefix}.server.{server_id}.>` | `>` | All methods for one backend | `mcp-nats/src/server.rs:44` |
| 3 | MCP client transport | `{prefix}.client.{client_id}.>` | `>` | All client-lane traffic for peer | `one_client.rs` (via `mod.rs:205-207`) |
| 4 | Any operator / ACL template | `{prefix}.server.>` | `>` | Export all backends | `all_server.rs:14` |
| 5 | Any operator / ACL template | `{prefix}.client.>` | `>` | Export all clients | `all_client.rs` |
| 6 | JetStream stream `MCP_AUDIT` | `{prefix}.audit.>` | `>` | Durable audit capture | `audit.rs:130` |
| 7 | Traffic projector (registry) | `mcp.audit.registry.>` | `>` | Registry audit fan-in | `trogon-traffic-view/src/projector/mod.rs:17` |
| 8 | Trust bundle KV watch | `>` (KV watch API) | `>` | All trust domains in bucket | `cache.rs:183-186` |
| 9 | **proposed** Anomaly scorer | `mcp.metrics.anomaly.*` | `*` | Per-tenant feature tap | — |
| 10 | **proposed** Gateway config watcher | KV watch on `mcp-gateway-config` | `>` | Policy reload | [bootstrap-day-zero.md](bootstrap-day-zero.md) |

Single-token `*` example (ACL): `{prefix}.gateway.request.*.tools.call` matches one server id segment — use in account exports; gateway itself subscribes with `>`.

---

## 5. Queue groups

Queue groups load-balance request/reply and queue subscribers across replicas. Plain subscribers (no queue group) receive every message.

| Service | Queue group | Env override | Subscribes / serves | Worker model | Source |
|---|---|---|---|---|---|
| `trogon-mcp-gateway` | `mcp-gateway` | `MCP_GATEWAY_QUEUE_GROUP` | `{prefix}.gateway.request.>` | Competing consumers; each message handled once | `config.rs:8`, `100-101`, `122-124`, `gateway.rs:71-72` |
| `trogon-sts` | `trogon-sts` | `MCP_STS_QUEUE_GROUP` | `mcp.sts.exchange` | Competing STS workers | `trogon-sts/src/lib.rs:26`, `main.rs:72-73` |
| `trogon-agent-registry` | `trogon-agent-registry` | — | `mcp.registry.agent.lookup` | Competing lookup workers | `consumer.rs:18`, `107-108` |
| `trogon-jwks-publisher` | `trogon-jwks-publisher` | — | `mcp.jwks.mesh.get` | JWKS req/rep pool | `jwks.rs:9` |
| MCP server (`mcp-nats`) | *(none)* | — | `{prefix}.server.{server_id}.>` | Every replica receives all messages unless operator adds queue group externally | `server.rs:44` |
| Approvals waiter | *(none)* | — | `mcp.approvals.{request_id}` | Single gateway instance subscribes per parked request | `approvals/client.rs:42` |
| Metrics publishers | *(none)* | — | fire-and-forget publish | N/A | `anomaly.rs:48` |

---

## 6. JetStream streams

### 6.1 Durable streams (implemented)

| Stream name | Default | Subject filter | Retention / limits (code defaults) | Publisher | Source |
|---|---|---|---|---|---|
| `MCP_AUDIT` | `MCP_GATEWAY_AUDIT_STREAM` | `{prefix}.audit.>` | `max_messages: 100_000`; other fields `Default` | Gateway audit | `audit.rs:125-137`, `config.rs:103-104` |

Stream creation is optional: `MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT=1` or `--skip-audit-stream-init` — `config.rs:106-107`, `129-138`.

Init failure is non-fatal (warn + continue) — `gateway.rs:83-86`.

Publish uses JetStream publish with ack timeout 5 s — `audit.rs:142-163`, `gateway.rs:574`.

### 6.2 Core NATS (ephemeral publish)

These audit and metrics subjects use **core** `client.publish` (no stream guarantee in application code):

| Family | Examples | Publisher | Source |
|---|---|---|---|
| STS audit | `mcp.audit.sts.*` | `trogon-sts` | `audit.rs:78-86` |
| Registry audit | `mcp.audit.registry.*` | `trogon-agent-registry`, controller | `audit.rs:54-64`, controller `audit.rs` |
| Anomaly features | `mcp.metrics.anomaly.features` | Gateway | `anomaly.rs:48` |
| Shadow metrics | `mcp.metrics.gateway.aud_mismatch_shadow` | Gateway | `egress/audience.rs:85` |
| STS probe metrics | `mcp.metrics.sts.latency` | STS probe | `probe.rs` |

Operators may attach JetStream sources or stream captures for these subjects out-of-band; the binaries do not create those streams today.

### 6.3 Traffic projector consumption

| Consumer | Stream | Filter | Source |
|---|---|---|---|
| `agent-traffic-projector` | `MCP_AUDIT` (default) | `mcp.audit.>` | [agent-traffic.md §2](agent-traffic.md#2-inputs), `trogon-traffic-view` |

Gateway-shaped `{prefix}.audit.{outcome}.{direction}.{method_root}` messages are normalized in `normalize.rs` but skipped by `consumer.rs` until handler widens — [integration-touchpoints.md §8](integration-touchpoints.md#8-gateway--traffic-indexer-and-ocsf-exporter).

---

## 7. KV buckets

KV entries live in JetStream KV buckets. **Bucket name ≠ NATS subject.**

### 7.1 `mcp-trust-bundles`

| Field | Value | Source |
|---|---|---|
| Constant | `TRUST_BUNDLES_KV_BUCKET` = `mcp-trust-bundles` | `trogon-sts/src/lib.rs:30` |
| Key format | `{trust_domain}` (e.g. `acme.local`, `local`) | `publish_trust_bundle.rs:44`, `cache.rs:144-149` |
| Value | PEM SPIFFE trust bundle (UTF-8 text) | `cache.rs:142-143` |
| TTL | None (JetStream KV default) | bucket create `..Default::default()` — `publish_trust_bundle.rs:37-40` |
| Max value size | JetStream KV default (not overridden in publish binary) | `publish_trust_bundle.rs:37-40` |
| Writers | `trogon-sts-publish-trust-bundle`, operators (`nats kv put`) | `publish_trust_bundle.rs` |
| Readers | `trogon-sts` (`load_trust_bundle_from_kv`, `spawn_trust_bundle_watch`) | `cache.rs:127-204`, `main.rs:101` |
| Gateway | Does **not** read this bucket today | [integration-touchpoints.md §7](integration-touchpoints.md#7-gateway--trust-bundle-distribution) |

### 7.2 `mcp-jwks`

| Field | Value | Source |
|---|---|---|
| Bucket | `mcp-jwks` | `trogon-jwks-publisher/src/jwks.rs:6`, `trogon-sts/src/lib.rs:28` |
| Primary key | `mesh/current` (`JWKS_KV_MESH_KEY`) | `lib.rs:29` |
| Value | JSON JWKS (`Jwks` / RFC 7517) | `jwks.rs:11-33` |
| Writers | `trogon-jwks-publisher` | crate README |
| Readers | `trogon-sts` mesh JWKS load/watch | `main.rs:91-97` |
| Req/rep fallback | `mcp.jwks.mesh.get` | `jwks.rs:8` |

Bootstrap JWKS may use `kv://mcp-jwks/bootstrap/current` URL form — `trogon-sts/README.md`.

### 7.3 `mcp-agent-registry`

| Field | Value | Source |
|---|---|---|
| Bucket | `mcp-agent-registry` | `store.rs:14` |
| Key format | `{agent_id}/@latest` (runtime pointer) | `store.rs:93-95` |
| History | `10` revisions | `store.rs:87` |
| Max value size | `65_536` bytes | `store.rs:16`, `88` |
| TTL | None | `bucket_config()` defaults |
| Writers | `trogon-agent-registry-controller` (production); dev `AgentRegistryStore::put/delete` | controller README, `store.rs:147-165` |
| Readers | `trogon-agent-registry` lookup service (KV + cache); STS/gateway via **lookup subject**, not direct KV | `consumer.rs`, [integration-touchpoints.md §4](integration-touchpoints.md#4-gateway--agent-registry) |

### 7.4 **proposed** buckets

| Bucket | Key (examples) | Purpose | Source |
|---|---|---|---|
| `mcp-gateway-config` | policy bundles, route tables | Gateway policy reload | [mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md) |
| `mcp-sessions` | `{session_id}` | Session binding / ZedToken cache | [mcp-session-model.md](mcp-session-model.md) |

---

## 8. Sample end-to-end traces

All examples use default prefix `mcp`. Reply subjects (`_INBOX.*`) are core NATS inbox tokens, not part of the grammar below.

### 8.1 Allowed `tools/list` (ingress → egress → audit)

| Step | Direction | Subject | Notes |
|---|---|---|---|
| 1 | Client → gateway | `mcp.gateway.request.github.tools.list` | Queue group `mcp-gateway` delivers to one worker |
| 2 | Gateway → backend | `mcp.server.github.tools.list` | Rewrite — `subject.rs:27` |
| 3 | Gateway → JetStream | `mcp.audit.allow.request.tools` | `outcome=allow`, `direction=request`, `method_root=tools` — `gateway.rs:446-463`, `audit.rs:112-114` |

Audit envelope fields include `subject_in`, `subject_out`, `jsonrpc_method`, `identity_source` — `audit.rs:33-58`.

### 8.2 Denied `tools/call` (policy → audit, no egress)

| Step | Direction | Subject | Notes |
|---|---|---|---|
| 1 | Client → gateway | `mcp.gateway.request.github.tools.call` | |
| 2 | *(no backend request)* | — | SpiceDB deny or CEL gate — `gateway.rs:306` |
| 3 | Gateway → JetStream | `mcp.audit.deny.request.tools` | `audit_outcome=deny` — `gateway.rs:306`, `622` |
| 4 | Client ← gateway | reply inbox | JSON-RPC error (e.g. `-32107`) |

Optional parallel publish (non-blocking): `mcp.metrics.anomaly.features` — `anomaly.rs:33-50`.

### 8.3 Egress mesh mint + STS audit (gateway → STS → backend)

| Step | Direction | Subject | Notes |
|---|---|---|---|
| 1 | Client → gateway | `mcp.gateway.request.github.tools.call` | Bearer mesh JWT on ingress |
| 2 | Gateway → STS | `mcp.sts.exchange` (request/reply) | Egress minter — `egress/mint.rs:180`, `lib.rs:27` |
| 3 | STS → core NATS | `mcp.audit.sts.success` | On successful exchange — `exchange.rs:94`, `audit.rs:53-54` |
| 4 | Gateway → backend | `mcp.server.github.tools.call` | Downstream mesh JWT in headers — `gateway.rs:421-442` |
| 5 | Gateway → JetStream | `mcp.audit.allow.request.tools` | Backend OK — `gateway.rs:446-463` |

If exchange fails: `mcp.audit.sts.deny` or `mcp.audit.sts.error` depending on `StsError::audit_outcome()` — `error.rs:71-80`.

Registry lookup during ingress chain resolution (parallel path): request/reply `mcp.registry.agent.lookup` → audit `mcp.audit.registry.lookup.found|notfound|revoked` — `consumer.rs:86-97`.

---

## 9. Compatibility guarantees

Semver applies to **documented wire subjects** consumed by other services, operators, or ACL templates.

### 9.1 Stable (minor-version compatible)

Treat as stable contract; only additive changes in minor releases:

| Identifier | Stability notes |
|---|---|
| `{prefix}.gateway.request.{server_id}.{method_suffix}` | Ingress contract — `subject.rs`, README |
| `{prefix}.server.{server_id}.{method_suffix}` | Backend lane — `mcp-nats` parsing table |
| `{prefix}.client.{client_id}.{method_suffix}` | Client lane — `mcp-nats` |
| `{prefix}.audit.{outcome}.{direction}.{method_root}` | Audit taxonomy — `audit.rs:112-114` |
| `mcp.sts.exchange` | STS wire — ADR 0004 |
| `mcp.registry.agent.lookup` | Registry R/R — `registry.md` |
| `mcp.approvals.{request_id}` | HITL wait/publish — `approvals/types.rs` |
| KV `mcp-trust-bundles/{trust_domain}` | Trust distribution — `lib.rs:30` |
| KV `mcp-agent-registry/{agent_id}/@latest` | Registry pointer — `store.rs:93-95` |
| KV `mcp-jwks/mesh/current` | Mesh signing keys — `lib.rs:28-29` |

Changing any stable subject or bucket name is a **major** breaking change requiring migration notes and overlap window.

### 9.2 Internal / may change in minor releases

| Identifier | Reason |
|---|---|
| `mcp.metrics.*` (all current metrics subjects) | Observability-only; schemas versioned in JSON `schema` field |
| `mcp.audit.sts.latency_violation` | Probe tooling — `probe.rs:13` |
| Hardcoded `mcp` on approvals/metrics/STS when `MCP_PREFIX` ≠ `mcp` | Known gap — prefix unification may change literals |
| Audit envelope JSON fields | Additive fields allowed; renames require schema version bump |
| JetStream `MCP_AUDIT` limits | `max_messages: 100_000` bootstrap default — operators may tune stream config |
| `{prefix}.audit.response.*`, `{prefix}.audit.callback.*` | Parsed in traffic spec; **not emitted** by gateway today — [agent-traffic.md §2.1](agent-traffic.md#21-gateway-tool-traffic) |

### 9.3 **proposed** only (not semver-bound)

- `{prefix}.approvals.{request_id}.grant`
- `mcp.metrics.anomaly.risk`
- `mcp-gateway-config`, `mcp-sessions` KV buckets
- `{prefix}.metrics.gateway.{metric_name}` general pattern
- Prefix-aware approvals / STS / registry subjects

---

## 10. Quick reference matrix

| Pattern | Transport | Durable? |
|---|---|---|
| `{prefix}.gateway.request.>` | Core + queue group | No |
| `{prefix}.server.*` / `{prefix}.client.*` | Core pub/sub or R/R | No |
| `{prefix}.audit.>` | JetStream (`MCP_AUDIT`) | Yes |
| `mcp.sts.exchange` | Core R/R + queue group | No |
| `mcp.registry.agent.lookup` | Core R/R + queue group | No |
| `mcp.approvals.*` | Core pub/sub | No |
| `mcp.metrics.*` | Core pub/sub | No |
| `mcp.audit.sts.*`, `mcp.audit.registry.*` | Core pub/sub (today) | No (unless operator streams) |
| KV buckets | JetStream KV API | Yes (KV persistence) |

---

## 11. Cross-references

| Topic | Document |
|---|---|
| Identity model, STS hop | [overview.md](overview.md) |
| Audit envelope fields, projector filters | [agent-traffic.md](agent-traffic.md) |
| Integration owners, env vars, failure modes | [integration-touchpoints.md](integration-touchpoints.md) |
| STS request/response bodies | [sts-exchange.md](sts-exchange.md) |
| Registry lookup API | [registry.md](registry.md) |
| Approval JSON-RPC `-32107` envelope | [adaptive-access.md](adaptive-access.md) |

---

*Generated for MCP_GATEWAY_PLAN Block H. Verify against cited sources when upgrading crates.*
