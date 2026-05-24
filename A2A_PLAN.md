# A2A over NATS Plan

A NATS-native implementation of the [Agent2Agent (A2A) protocol](https://a2aproject.github.io/A2A/) — agent discovery, task lifecycle, streaming updates, and push notifications — running on NATS subjects and JetStream instead of HTTP/SSE/webhooks.

## Intent

A2A is JSON-RPC 2.0 over HTTPS with SSE for streaming and HTTP webhooks for push. The protocol design is transport-agnostic at the semantic layer: AgentCards, tasks, messages, artifacts, and the streaming task lifecycle are the contract. The transport is replaceable.

Provide a transport binding where:

- **Discovery** — AgentCards are published to and resolved from a NATS-backed catalog instead of `/.well-known/agent.json`.
- **Unary methods** — `message/send`, `tasks/get`, `tasks/cancel`, `tasks/pushNotificationConfig/*` map to NATS request/reply on per-agent subjects.
- **Streaming** — `message/stream` and `tasks/resubscribe` map to JetStream consumers on per-task event subjects; clients re-attach by replaying from a sequence.
- **Push notifications** — replaced by durable JetStream subscriptions on caller-owned subjects; the HTTP webhook becomes a NATS publish.
- **Identity** — A2A's "declared auth schemes in AgentCard" becomes a NATS auth callout that mints a scoped user JWT; the AgentCard advertises NATS-native schemes.

The result is opaque agent-to-agent collaboration with the same semantics as HTTP A2A, but with NATS' operational model: queue-group horizontal scaling, JetStream-backed task event retention, KV-backed discovery, and replay/resubscribe as a first-class primitive instead of a webhook bolt-on.

The same gateway and policy engine described in `MCP_GATEWAY_PLAN.md` apply on top — A2A is the second tenant of that engine.

**Documentation hub (operators + embedders):** [`docs/A2A_DOCS_INDEX.md`](docs/A2A_DOCS_INDEX.md) · env reference [`docs/A2A_RUNTIME_ENV.md`](docs/A2A_RUNTIME_ENV.md) · streaming back-pressure [`docs/A2A_STREAMING_BACKPRESSURE_OPS.md`](docs/A2A_STREAMING_BACKPRESSURE_OPS.md).

## Implementation Status

Snapshot of what is in-tree vs. what this plan still describes as future work.

### Shipped crates

- `rsworkspace/crates/a2a-nats` — protocol binding: subjects, JetStream stream/consumer/provision, JSON-RPC envelope, `Bridge` (agent runtime), **`Client`**, **`gateway_ingress`** helpers. Also ships in-crate modules for catalog (`catalog/`), push delivery (`push/`), and audit (`audit/`).
- `rsworkspace/crates/a2a-nats-agent` — daemon shell that drives a user-supplied `A2aHandler` against the `Bridge`. Ships a `NoopHandler` and an `examples/echo.rs` reference handler.
- `rsworkspace/crates/a2a-nats-server` — axum HTTP/SSE surface over **`a2a_nats::Client`**; **`A2A_USE_GATEWAY=1`** optionally targets **`{prefix}.gateway.{agent_id}.{method}`** for **`a2a-gateway`** (see crate README). Not an A2A↔HTTPS interop bridge yet — bridges foreign HTTPS clients separately.
- `rsworkspace/crates/a2a-nats-stdio` — line-delimited JSON-RPC stdio bridge for tools that cannot embed NATS.
- `rsworkspace/crates/a2a-nats-discovery` — standalone KV provisioning plus `DiscoverService` on `{prefix}.discover.*` and `CatalogRegistrarService` on `{prefix}.catalog.register.*` (catalog registrar process).
- `rsworkspace/crates/a2a-bridge` — Phase&nbsp;4 HTTPS↔NATS bridge runtime: axum HTTPS surface, auth-callout client, NATS publish/consume via `async-nats`, SSE↔JetStream framing, outbound HTTPS for NATS-in/HTTPS-out, trait-mocked tests. Production-mode wiring against a deployed auth-callout still future ([`docs/A2A_BRIDGE_SKETCH.md`](./docs/A2A_BRIDGE_SKETCH.md)).
- `rsworkspace/crates/a2a-auth-callout` — NATS subscriber surface for `$SYS.REQ.USER.AUTH`: OIDC JWKS verification, mTLS x509 chain verification, **HMAC-SHA256 API-key verifier** (transitional), Account-bound User JWT mint (`sub` external, `aud` = Account, SpiceDB principal in `data`, `caller_id` derived from sub+aud).
- `rsworkspace/crates/a2a-redaction` — Wasmtime engine + module loader + skill-id dispatch for Tier 3 redaction; ABI is `redact_part(in_ptr, in_len) -> (out_ptr, out_len)` over linear memory. Gateway request-path wiring still future.
- `rsworkspace/crates/a2a-gateway` — NATS-connected process with optional queue group on **`{prefix}.gateway.>`**; request/reply forward from gateway-shaped subjects to **`{prefix}.agent.{agent_id}.{method}`** (`a2a_nats::gateway_ingress`). Policy stack under `src/policy/`: **Tier-1 SpiceDB** (`SpiceDbTier1Gate` in `spicedb_tier1.rs`, env-gated via `A2A_GATEWAY_TIER1_SPICEDB_ENABLED`; dispatch order: unary deadline guard → Tier-1 → Tier-2 → forward), Tier 2 `Tier2CelEvaluator` trait + `NoopTier2Evaluator`, Tier 3 `a2a_redaction::wasm::WasmRedactorHost` when `A2A_GATEWAY_POLICY_BUNDLE_DIR` is set (`A2A_GATEWAY_POLICY_SKILLS` preloads named WASM bundles). Unary `message.send` enforces a deadline (`A2A_GATEWAY_UNARY_DEADLINE_SECS`, default `DEFAULT_OPERATION_TIMEOUT`) returning JSON-RPC `-32800` on overrun; Tier-1/Tier-2 policy denial returns `-32801`. Decision-site `AuditEnvelope` JSON is published when `A2A_GATEWAY_AUDIT_PUBLISH` is truthy (`trace_id`, `rules_fired`, `rewrites`, `stream_consumer`, and `zed_token_snapshot` populated). Caller attribution rides `X-A2a-Caller-Id` on ingress. Env-gated durable JetStream pull egress on **`A2A_EVENTS`** when **`A2A_GATEWAY_EVENTS_PULL=on`** (`gw_pull_backpressure`). Env-gated push DLQ mirror on **`{prefix}.push.dlq.>`** when **`A2A_GATEWAY_PUSH_DLQ_MIRROR=on`**. Auth-callout JWT verification at ingress remains future. The **`planned::`** submodule collects **compile-only** roadmap seams for remaining Phase 2+ items.
- `rsworkspace/crates/a2a-pack` — packaging surface: **AgentCard JSON Schema landed** (`agent_card_schema::validate_agent_card_value`, enforced by `KvCatalogStore` on get/put). Other bundle modules (tuples, audit envelope extensions, redaction rules, rate-limit profiles) remain placeholders.

### Working surface (Phase 2 partial)

- Unary methods: `message/send`, `tasks/get`, `tasks/list`, `tasks/cancel`, `tasks/pushNotificationConfig/{set,get,list,delete}`, `agent/getAuthenticatedExtendedCard`. Each routes through `a2a.agent.{agent_id}.{method}` to an `A2aHandler` impl.
- Streaming: `message/stream` (bootstrap reply + JetStream pump) and `tasks/resubscribe` (RPC for snapshot + JetStream consumer at `last_seq + 1`). Backed by a single `A2A_EVENTS` stream filtered to `a2a.task.*.events.*`.
- Backpressure: `Config::max_concurrent_client_tasks` enforced via a Semaphore in the `Bridge` dispatch loop.
- Consumer hygiene: ephemeral consumers carry `inactive_threshold = 5m` so client crashes don't leak server state.
- Push delivery (partial): on terminal `TaskStatusUpdateEvent`, the `message/stream` event pump calls `dispatch_push_notifications` via `CompositePushDispatcher` — **HTTP(S)** (Bearer/Basic/jwt→Bearer; bounded retries on retryable response codes and transient transport failures), core NATS **`subject:{…}`**, and JetStream **`jetstream:{…}`** (ack’d publish; bounded retries with the same attempt cap / backoff as HTTPS on publish + ack failures; stream must pre-exist with a matching subject filter). **`provision_streams`** creates **`A2A_EVENTS`** plus **`A2A_PUSH_DLQ`**; **`push::dlq`** JetStream-publishes JSON failure envelopes (`schema`=`a2a.push.dlq/v1`) after terminal dispatch failures, with the DLQ **`caller_id`** segment derived from the minted JWT principal (`CallerId::from_principal` over `SpiceDbPrincipal`; falls back to `_` only when the principal omits `spicedb_subject`). **`a2a-gateway`** does **not** emit DLQ traffic. Digest signing / fuller auth schemes remain open.
- Agent catalog (partial): `CatalogStore` / `KvCatalogStore` over `A2A_AGENT_CARDS`, plus `DiscoverService` on `{prefix}.discover.*` request/reply and **`CatalogRegistrarService` on `{prefix}.catalog.register.{agent_id}`** (NATS `request` writes). The `rsworkspace/crates/a2a-nats-discovery` binary provisions the KV bucket and runs both services. **`KvCatalogStore` applies the bundled `a2a-pack` AgentCard schema on KV get/put.** Tenant-scoped discovery and **ACL-only enforcement** that only the registrar Principal may hit `catalog.register` remain operator work.
- Audit (partial): `Bridge::dispatch` emits one `AuditEnvelope` per handled RPC (`AuditEmitter`; default noop). **`message/stream` pump emits `TaskLifecycleEnvelope` payloads to `{prefix}.audit.lifecycle` when streamed task numeric state transitions** (`NatsAuditEmitter`). Gateway policy attribution fields (`rules_fired`, etc.) remain future once a gateway path exists.

### Intentional simplifications vs. the design below

- **No multi-Account provisioning yet.** Subjects already match the decided Account-scoped shape — operationally everything runs inside a single shared NATS Account today. Outstanding work is NSC operator/account provisioning and auth-callout deployment.
- **No gateway auth callout yet.** The auth-callout crate ships verifiers (OIDC, mTLS, HMAC API-key) and a JWT minter, but a deployed subscriber on `$SYS.REQ.USER.AUTH` is operator work. The gateway runs **Tier-1 SpiceDB** (`A2A_GATEWAY_TIER1_SPICEDB_ENABLED`), Wasmtime substrate as a **Tier-2 predicate seam** (`NoopTier2Evaluator` until CEL→WASM lands), and emits decision-site `AuditEnvelope` JSON with `trace_id`, `rules_fired`, `rewrites`, `stream_consumer`, and `zed_token_snapshot` when `A2A_GATEWAY_AUDIT_PUBLISH` is on; an authoritative JWT-derived `caller_id` is still future. Authoritative Tier-3 redaction at the request path and bundle signing remain to be built.
- **Push delivery is terminal-status-only; targets are HTTP, core NATS `subject:` and JetStream `jetstream:` publishes.** Caller-provisioned streams must bind the JetStream subjects. In-process retries with backoff ship for **HTTPS and for NATS / JetStream push publishes** (**`constants::HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS`** caps attempts for all three; HTTPS additionally gates retries by status code / error kind). **`provision_streams`** creates **`A2A_PUSH_DLQ`**, and the agent **`Bridge`** publishes DLQ JSON on terminal push delivery failures (see **`push::dlq`**). **Exactly-once opt-in** ships (`DeliverySemantics` JSON-RPC extension + `PushDeliverySemanticsRegistry`, `PushIdempotencyKey` + `IdempotencyKeyHeader`); JWT-derived **`caller_id`** DLQ segments ship via `CallerId::from_principal` (`_` fallback only when the minted JWT principal omits `spicedb_subject`). Optional gateway-side DLQ mirroring remains to be built (Digest webhook auth deferred per §Decisions).
- **Agent catalog is partial.** `catalog/` plus `a2a-nats-discovery` provide KV-backed cards, `{prefix}.discover.*` reads, and **`{prefix}.catalog.register.*` writes** into KV. **`a2a-pack` ships the minimal JSON Schema**; **`KvCatalogStore`** enforces it on **KV get/put** (registrar path uses the same validator). Remaining gaps: **enforced** registrar-only ACL posture in every environment, gateway / edge AgentCard flows that bypass KV reads, federation-shaping, KV watch ergonomics, and fuller registration policy.
- **No HTTPS↔NATS interop bridge listeners yet** for foreign A2A clients. The **`a2a-bridge`** crate is types-only today; **`a2a-nats-server`** exposes HTTP for the embedded Rust **`a2a_nats::Client`**, not generic HTTPS-Agent interop.

### Crates this plan still calls for

The following remain separate deliverables. Catalog, push, and audit *modules* already live inside `a2a-nats`; `a2a-nats-discovery` covers KV ownership + discover request/reply, but gateway integration is still open.

- **`a2a-gateway` / `a2a-pack`** — `a2a-gateway` connects and relays ingress; authz/policy bundles consume `a2a-pack`, which currently ships placeholder modules. Bundles (resource tuples, AgentCard schema, redaction rules, audit envelope) remain to be built per §Decisions.
- **`a2a-bridge`** — HTTPS↔NATS interop sidecar in both directions (Phase&nbsp;4). Cargo stub lands config types now; terminating ingress, minted User JWT client, SSE↔JetStream mapping remain to be shipped.
- Registrar hardening beyond `a2a-nats-discovery` — AgentCard JSON-Schema on write + KV watch ergonomics documented for clients.

## Inspiration

### [Google A2A](https://a2aproject.github.io/A2A/)

The reference. We adopt the data model wholesale:

- **AgentCard** — capability advertisement; supported transports, skills, auth schemes, extensions.
- **Task** — the unit of work, with a strict state machine (`submitted` → `working` → `input-required` → `completed` / `failed` / `canceled` / `rejected`).
- **Message / Part / Artifact** — structured payloads with `text`, `file`, `data` parts; tasks emit artifacts.
- **PushNotificationConfig** — caller-supplied delivery target for task updates.
- **JSON-RPC 2.0 method surface** — `message/send`, `message/stream`, `tasks/get`, `tasks/cancel`, `tasks/resubscribe`, `tasks/pushNotificationConfig/set|get|list|delete`, `agent/getAuthenticatedExtendedCard`.

What we change is the transport binding, not the semantics. A NATS-bound A2A client and an HTTPS-bound A2A client are interoperating through a bridge.

### A2A's existing transport flexibility

The A2A spec already acknowledges JSON-RPC, gRPC, and HTTP+JSON as parallel transports with the same semantics — the [protocol docs](https://a2aproject.github.io/A2A/latest/specification/) explicitly list "Transport Layer Requirements" as pluggable. NATS becomes a fourth peer transport, not a non-standard fork.

### [MCP Gateway Plan](./MCP_GATEWAY_PLAN.md)

The architectural template. A2A's transport binding is intentionally shaped like MCP's so the gateway, auth callout, policy engine, audit pipeline, and bundle distribution are shared.

- Two-namespace separation (`a2a.gateway.{tenant}.>` vs. `a2a.agent.{id}.>`) is the same shape as MCP's.
- The policy engine (Tier 1 declarative / Tier 2 CEL / Tier 3 WASM) is reused — only the resource-tuple derivation table differs.
- Audit envelopes, bundle signing, hot-swap, and SpiceDB integration are unchanged.

### [agentgateway](https://github.com/agentgateway/agentgateway)

Same caveats as the MCP plan: validates the demand and the policy-engine choices; HTTP-centric, not NATS-native.

## Protocol Mapping

### Tenancy model

**Tenant = NATS Account** *(decided 2026-05-23, see `A2A_PENDING_DECISION.md`)*. Each tenant runs in a dedicated NATS Account; subjects below have **no `{tenant}` segment** because Account membership is the tenancy boundary. Cross-tenant traffic requires explicit operator-signed Account exports/imports — federation is opt-in. JetStream streams and KV buckets use the same names across Accounts (`A2A_EVENTS`, `A2A_AGENT_CARDS`); the Account namespace disambiguates.

### Subject layout

Two namespaces separated by NATS authorization, **scoped inside each Account**:

| Subject                                       | Role                                                          | Who publishes              |
|-----------------------------------------------|---------------------------------------------------------------|----------------------------|
| `a2a.gateway.{agent_id}.{method}`             | Client → gateway request inbox                                | Clients                    |
| `a2a.agent.{agent_id}.{method}`               | Gateway → agent backend                                       | Gateway                    |
| `a2a.task.{task_id}.events.{req_id}`          | Per-task event stream (JetStream)                             | Agent (via gateway)        |
| `a2a.push.{caller_id}.{task_id}`              | Caller-owned push delivery subject                            | Gateway                    |
| `a2a.discover.{agent_id}`                     | AgentCard catalog (KV-backed; also a service subject)         | Registrars                 |
| `a2a.catalog.register.{agent_id}`             | AgentCard catalog **write** (registrar `request` → KV put)     | Registrars (ACL-gated)     |

`{method}` is the JSON-RPC method with `/` replaced by `.` — `message.send`, `message.stream`, `tasks.get`, `tasks.cancel`, `tasks.resubscribe`, `tasks.pushNotificationConfig.set`, etc.

### AgentCard discovery

A2A's `/.well-known/agent.json` is replaced by a NATS KV bucket `A2A_AGENTCARDS_{tenant}` keyed by `{agent_id}`. Two access patterns:

- **Pull** — clients fetch via KV `get` with optional revision watch.
- **Service-style** — `a2a.discover.{tenant}.{agent_id}` is a request/reply endpoint that returns the AgentCard, suitable for clients that don't speak KV.

#### Why KV, not scatter-gather

Scatter-gather (the NATS Services `$SRV.PING` / `$SRV.INFO` pattern) is the reflex NATS-idiomatic answer for "who's online?" and was considered. We chose KV instead. The reasoning, recorded so it doesn't get re-litigated in review:

- **AgentCards are semi-static config, not runtime state.** Skills, schemas, examples, auth schemes change on deploy, not constantly. KV is shaped for write-rarely, read-often, watch-for-changes.
- **KV watchers beat re-scattering.** Discovery clients `watch` the bucket once and get push updates on change. Scatter-gather forces a fresh fan-out on every query.
- **Fan-out cost scales with fleet size.** N agents × every discovery = N replies per query, plus a dedupe layer for queue-group replicas. KV `get` is O(1).
- **Authz shaping caches better against a stable set.** `BulkCheckPermission` on a known agent id list reuses ZedToken cache; on a dynamic gather result it doesn't, and races against agents joining/leaving mid-gather.
- **Semantic parity with HTTPS A2A.** The spec's discovery model is *pull on demand* (`/.well-known/agent.json`). KV `get` matches one-to-one; scatter-gather is a behavior delta the HTTPS bridge would have to paper over.
- **Liveness from PING is theatre on the discovery path.** "Agent responded to PING" ≠ "agent will accept the next message". The request/reply on `a2a.agent.{id}.message.send` is the real liveness signal — and it's the call the client was about to make anyway.

Scatter-gather earns a place later as a `$SRV.PING`-style health subject for the gateway to monitor agent health (Phase 4 ops tooling), not as the discovery primitive.

AgentCards advertise NATS as a transport:

```json
{
  "transports": [
    {
      "kind": "nats",
      "endpoint": "nats://a2a.gateway.acme",
      "agent_subject": "a2a.gateway.acme.support-bot",
      "auth_schemes": [
        { "type": "nats-callout", "issuer": "acme-trogon" },
        { "type": "nkey" }
      ]
    },
    {
      "kind": "jsonrpc-https",
      "url": "https://acme.example.com/a2a"
    }
  ]
}
```

`agent/getAuthenticatedExtendedCard` is a request/reply against the gateway namespace with the caller's JWT — same response shape as A2A specifies.

### Unary methods (request/reply)

```
client ──PUB──▶ a2a.gateway.{tenant}.{agent_id}.message.send  (JSON-RPC envelope, reply_to = _INBOX.x)
                        │
                        ▼
                ┌──────────────────┐
                │  Gateway service │
                └──────────────────┘
                        │
        1. parse JSON-RPC (method, params, id)
        2. resolve identity from JWT claims
        3. derive resource tuple (see SpiceDB table)
        4. policy.authorize(req)
        5. policy.rewrite(req.params)
        6. publish a2a.agent.{agent_id}.message.send
        7. await reply on inbox
        8. policy.rewrite(resp.result)
        9. emit audit
       10. reply to client on _INBOX.x
```

### Streaming (`message/stream`, `tasks/resubscribe`)

A2A's SSE stream becomes a JetStream consumer:

- On `message/stream`, the gateway:
  1. allocates `task_id` (or reuses the one in `params.message.taskId`),
  2. ensures a JetStream stream `A2A_TASKS_{tenant}` with subject filter `a2a.task.{tenant}.>` exists,
  3. forwards the request to the agent on `a2a.agent.{agent_id}.message.stream` with `events_subject = a2a.task.{tenant}.{task_id}.events`,
  4. creates an ephemeral push consumer for the caller bound to that subject filter, delivering to a caller inbox,
  5. emits an immediate JSON-RPC ack containing `{task_id, stream_inbox, last_seq: 0}`.

The agent publishes each `Task` / `TaskStatusUpdateEvent` / `TaskArtifactUpdateEvent` to `a2a.task.{tenant}.{task_id}.events`. The gateway consumer rewrites/redacts in flight and forwards to the caller inbox.

`tasks/resubscribe` becomes a new ephemeral consumer on the same subject starting from `OptStartSeq = last_seq + 1` — the durable equivalent of SSE reconnect, but lossless within JetStream retention.

### Push notifications

`tasks/pushNotificationConfig/set` registers a delivery target. The NATS binding accepts:

- **NATS subject** — `subject: "a2a.push.acme.caller-42.task-9"` — gateway publishes the same envelope SSE would carry.
- **JetStream stream + subject** — same, but the caller pre-creates a durable stream for at-least-once delivery across disconnects.
- **HTTPS webhook** — preserved for cross-binding interop; the gateway acts as a webhook client.

Push deliveries use the same JSON envelope as the SSE stream; payload semantics are unchanged. Auth on the push target follows A2A's existing `pushNotificationAuthenticationInfo` model (bearer / signed JWT). For NATS targets, identity is the gateway's own NATS user.

### Method → subject table

| JSON-RPC method                                  | Subject (client → gateway)                                              | Direction       |
|--------------------------------------------------|-------------------------------------------------------------------------|-----------------|
| `message/send`                                   | `a2a.gateway.{tenant}.{agent_id}.message.send`                          | req/reply       |
| `message/stream`                                 | `a2a.gateway.{tenant}.{agent_id}.message.stream`                        | req → stream    |
| `tasks/get`                                      | `a2a.gateway.{tenant}.{agent_id}.tasks.get`                             | req/reply       |
| `tasks/cancel`                                   | `a2a.gateway.{tenant}.{agent_id}.tasks.cancel`                          | req/reply       |
| `tasks/resubscribe`                              | `a2a.gateway.{tenant}.{agent_id}.tasks.resubscribe`                     | req → stream    |
| `tasks/pushNotificationConfig/set|get|list|delete` | `a2a.gateway.{tenant}.{agent_id}.tasks.pushNotificationConfig.{op}`     | req/reply       |
| `agent/getAuthenticatedExtendedCard`             | `a2a.gateway.{tenant}.{agent_id}.agent.getAuthenticatedExtendedCard`    | req/reply       |

## Architecture

### Components

All components run **inside the tenant's NATS Account**; cross-Account traffic is gated by operator-signed exports/imports.

1. **Auth callout service** — terminates external OIDC / mTLS / API keys and mints a **User JWT bound to the tenant's Account**. Subject ACL inside the Account confines the caller to `a2a.gateway.>` and an `_INBOX.{caller_id}.>` reply space, plus consumer ACL on caller-owned push subjects.
2. **A2A gateway service** — queue-group subscriber on `a2a.gateway.>` inside the Account; stateless per-message except for streaming where it owns the consumer-to-caller pipe.
3. **Discovery / catalog service** — owns the Account's `A2A_AGENT_CARDS` KV bucket; serves `a2a.discover.>` requests. Registrars publish AgentCards; gateway validates against AgentCard JSON Schema before accepting. Federated discovery (across Accounts) requires explicit Account exports of `a2a.discover.>`.
4. **Task stream manager** — ensures the Account's `A2A_EVENTS` JetStream stream exists with subject filter `a2a.task.>`, configures retention (default `interest` with `max_age = 24h`, per-Account override), provisions consumers.
5. **Policy engine** — reused from MCP plan; A2A resource-tuple derivation table layered as a bundle. Policies evaluate inside the Account; cross-Account SpiceDB principals carry Account identity.
6. **Audit emitter** — `a2a.audit.{outcome}.{method}` inside the Account (Account namespace = tenant attribution).
7. **HTTP↔NATS bridge (optional, sidecar)** — accepts vanilla A2A HTTPS clients on one side, mints an Account-bound NATS User JWT, publishes inside the caller's tenant Account on the other. Lets external agents that only speak HTTP A2A participate without code change.

### Backend agents

Agents subscribe to `a2a.agent.{agent_id}.>` as a queue group. Same pattern as `mcp-nats` backend servers. An A2A SDK adapter handles:

- JSON-RPC envelope decode,
- task state machine bookkeeping,
- publishing task events to the per-task subject the gateway hands them.

### Streaming flow

```
caller ──PUB──▶ a2a.gateway.{tenant}.{agent}.message.stream
                        │
                        ▼
              ┌────────────────────┐
              │  Gateway           │
              │  - authz           │
              │  - alloc task_id   │
              │  - ensure stream   │
              │  - create consumer │
              └────────────────────┘
                        │
        forward ──PUB──▶ a2a.agent.{agent}.message.stream
                                        │
                                        ▼
                              ┌─────────────────┐
                              │  Agent backend  │
                              └─────────────────┘
                                        │
                  publish events ──PUB──▶ a2a.task.{tenant}.{task_id}.events
                                        │
                ┌───────────────────────┘
                ▼
          Gateway consumer ──rewrite / shape / audit──▶ caller stream inbox
```

Caller disconnect: ephemeral consumer dies; on `tasks/resubscribe` the gateway recreates it at `last_seq + 1`. Lossless within retention window. Push-notification targets see the same events independently via their own durable consumer.

## Auth Model

**Tenant boundary = NATS Account** *(decision 2026-05-23)*. Each tenant runs in a dedicated Account; auth produces an Account-bound User JWT, and subject ACLs are written *inside* that Account.

AgentCards declare `auth_schemes` per transport. NATS-bound schemes:

- **`nats-callout`** — external OIDC / mTLS / API key terminates at a callout service that mints a **User JWT bound to the tenant's Account**. Subject ACL inside the Account bounds the caller to `a2a.gateway.{agent_id}.>` and an `_INBOX.{caller_id}.>` reply space.
- **`nkey`** — direct NKey-based User inside the tenant's Account for service-to-service agents.
- **`jwt-bearer`** — passthrough JWT carried in the JSON-RPC envelope `metadata`, validated by the gateway against AgentCard-declared issuers; Account membership still required at the connection layer.

Cross-binding: when the HTTPS↔NATS bridge accepts an HTTPS A2A client, it terminates HTTP auth and re-mints a NATS **User JWT in the caller's tenant Account**. AgentCard's `security` block stays HTTPS-shaped on the outside; NATS-bound peers see the NATS-native schemes. Cross-tenant access is only possible through operator-signed Account exports — off by default.

## Policy Engine

Reuse the engine from `MCP_GATEWAY_PLAN.md` verbatim (three tiers, WASM substrate, signed bundles, hot-swap, audit-to-NATS). The A2A bundle ships:

- Resource-tuple derivation for A2A methods (table below).
- Catalog shaping rules — filter AgentCards in the `a2a.discover` response by caller permission (analogue of MCP's `tools/list` shaping).
- Schema-driven redaction over `Message.parts[*]` and `Artifact.parts[*]`, indexed by AgentCard skill definitions.
- Per-task quota and rate-limit policies (e.g., max concurrent streaming tasks per caller per agent).
- Audit envelope schema for A2A.

### SpiceDB resource tuples

| A2A method                                       | Subject              | Permission             | Resource                                     |
|--------------------------------------------------|----------------------|------------------------|----------------------------------------------|
| `agent/getAuthenticatedExtendedCard`             | `user:{sub}`         | `view`                 | `agent:{agent_id}`                           |
| `discover` (catalog list)                        | `user:{sub}`         | `view`                 | `agent:{agent_id}` (bulk-checked)            |
| `message/send`                                   | `user:{sub}`         | `invoke`               | `agent:{agent_id}`                           |
| `message/stream`                                 | `user:{sub}`         | `invoke_stream`        | `agent:{agent_id}`                           |
| `tasks/get`                                      | `user:{sub}`         | `read`                 | `task:{task_id}`                             |
| `tasks/cancel`                                   | `user:{sub}`         | `cancel`               | `task:{task_id}`                             |
| `tasks/resubscribe`                              | `user:{sub}`         | `read`                 | `task:{task_id}`                             |
| `tasks/pushNotificationConfig/*`                 | `user:{sub}`         | `configure_push`       | `task:{task_id}`                             |

Task ownership is established on the `message/send` or `message/stream` that creates the task — gateway writes a `task:{id}#owner@user:{sub}` relationship to SpiceDB at task creation, removes it at terminal state (or retention boundary).

`BulkCheckPermission` is used for catalog shaping on the discovery endpoint. ZedToken consistency scoped per-session. **Federated import boundary:** `a2a-nats::catalog::import_gate::SpiceDbImportGate` issues `CheckBulkPermissions` against `agent_card:{publisher}:{agent_id}` (env-gated; deny-only when unconfigured). **Gateway Tier 1** request-path SpiceDB ships in `a2a-gateway/src/policy/spicedb_tier1.rs` (env-gated; per-method tuples, session ZedToken cache, owner tuples on `message/send` accept).

## Audit

`a2a.audit.{tenant}.{outcome}.{method}` envelopes mirror the MCP audit shape, with A2A-specific fields:

```json
{
  "ts": "2026-05-22T10:00:00Z",
  "trace_id": "...",
  "tenant": "acme",
  "caller": { "sub": "user:alice", "via": "oidc:google" },
  "method": "message/stream",
  "agent_id": "support-bot",
  "task_id": "t_abc123",
  "task_state_before": "submitted",
  "task_state_after": "working",
  "decision": "allow",
  "rules_fired": ["a2a-invoke-stream-authz", "redact-message-parts"],
  "rewrites": [{ "path": "$.params.message.parts[0].text", "op": "classify:pii.email" }],
  "stream_consumer": "A2A_TASKS_acme/c_x9",
  "latency_us": 1820
}
```

Per-task lifecycle is recoverable from audit alone: every state transition emits an envelope. SIEM consumers subscribe via durable JetStream consumer.

## Bundles

Same packaging, signing, distribution, and hot-swap as the MCP plan. The first-party "A2A pack" ships:

- Resource-tuple table.
- Catalog shaping for AgentCard discovery.
- AgentCard JSON Schema validation (registration-time guard).
- Schema-driven redaction over `parts[*]` keyed by skill id.
- Task lifecycle audit shape.
- Default rate-limit profile per skill kind.

Tenants extend by overlaying bundles — same composition model as MCP.

## Interop Bridge

A standalone `a2a-bridge` service makes the NATS binding interoperable with the HTTPS A2A ecosystem in both directions:

- **HTTPS in, NATS out** — terminates standard A2A HTTPS, translates JSON-RPC to NATS publish, streams SSE back from the JetStream consumer.
- **NATS in, HTTPS out** — registers external HTTPS agents into the NATS catalog as proxied AgentCards; gateway forwards to the HTTPS endpoint and adapts SSE → JetStream events.

Auth is re-minted at the bridge in both directions. AgentCards declare both transports so polyglot clients pick.

## Decisions

All architectural decisions are landed.

- **Tenancy = NATS Account per tenant** (2026-05-23). Cross-Account traffic requires explicit operator-signed exports/imports; federation is opt-in. Subject table in §Protocol Mapping carries no `{tenant}` segment; stream/KV names are reused across Accounts.
- **Stream topology = shared `A2A_EVENTS` per Account, subject-filtered `a2a.task.>`**. Per-task streams rejected — JS metadata cost dominates at fleet scale.
- **Retention default = 24h baseline, per-Account override**. JetStream `max_age = 24h`.
- **Push delivery semantics = at-least-once default**, exactly-once as opt-in flag on `PushNotificationConfig`.
- **AgentCard write path = central registrar (`a2a-nats-discovery`) validates against JSON-Schema, then writes KV.** Schema lives in `a2a-pack`; gateway re-validates on read.
- **Streaming back-pressure = gateway pull consumer with flow control + stream policy `retention=interest, discard=old` (shipped env-gated in `a2a-gateway`; full ingress pipe still future).** Agents never block on publish.
- **`message/send` max blocking window = 30s.** Longer work forced onto `message/stream`. Gateway enforces this via `A2A_GATEWAY_UNARY_DEADLINE_SECS` (default `DEFAULT_OPERATION_TIMEOUT`); overrun replies JSON-RPC `-32800`.
- **Bridge identity = auth-callout mints per-request User inside the caller's tenant Account.** Single-bridge-User rejected to preserve caller attribution in audit.
- **Federated discovery = off by default; opt-in via operator-signed exports of `a2a.discover.>`**; SpiceDB gates the import side.
- **Cross-process push DLQ = per-Account `A2A_PUSH_DLQ` JetStream stream** at `a2a.push.dlq.{caller_id}.{task_id}`.
- **Digest webhook auth = deferred.** Currently rejected with a clear error.
- **NATS-push auth = gateway NATS User + subject ACL on `a2a.push.>`** (gateway publish) and `a2a.push.{caller_id}.>` (caller read).
- **Audit envelope = plan §Audit shape verbatim, no `tenant` field** (Account namespace carries the attribution).
- **Auth callout = NATS subscriber on `$SYS.REQ.USER.AUTH`**; OIDC primary, mTLS for service-to-service, API keys transitional. Mints Account-bound User JWT.
- **Policy substrate = single Wasmtime runtime in the gateway** hosting Tier 2 (CEL compiled to WASM at bundle build) and Tier 3 (WASM redaction).
- **SpiceDB = org-standard cluster, gateway holds the client.** Resource tuples per §SpiceDB.
- **`a2a-bridge` = HTTPS sidecar; per-request re-mint into caller's Account; SSE↔JetStream consumer mapping** on `a2a.task.{task_id}.events.>`.

## Phased Delivery

Aligned with the MCP plan's phases so they share infrastructure.

- **Phase 0** *(partial)* — Account provisioning + auth callout + subject ACL + AgentCard KV bucket + unary `message/send` and `tasks/get` end-to-end. No streaming, no push, no policy. Prove the perimeter and the catalog.
  - Done: unary `message/send`, `tasks/get`, and the rest of the unary surface; in-crate `catalog/` (`CatalogStore`, `KvCatalogStore`, `DiscoverService`, `CatalogRegistrarService`); standalone registrar binary (`a2a-nats-discovery`) provisions `A2A_AGENT_CARDS`, serves `{prefix}.discover.*`, and accepts `{prefix}.catalog.register.*` writes. Subject layout already matches the Account-per-tenant decision (no `{tenant}` segment).
  - Pending: NSC operator/account provisioning runbook (**outline:** [`docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md`](./docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md)); JetStream asset cheat sheet (**[`docs/A2A_JETSTREAM_ACCOUNT_STREAMS.md`](./docs/A2A_JETSTREAM_ACCOUNT_STREAMS.md)**); the auth-callout crate ships verifiers (OIDC, mTLS, HMAC API-key) and JWT mint — **deployed subscriber on `$SYS.REQ.USER.AUTH`** is operator work; subject ACL inside each Account via deployed NSC templates (`scripts/acl-templates/` ships caller/gateway/registrar templates); **`catalog.register` publish restricted to registrar User** via deployed ACLs (not just documentation); KV watch ergonomics for clients.
- **Phase 1** *(partial)* — Tier 1 policies, SpiceDB authz on `message/send` and `tasks/*`, audit to JetStream. Discovery shaping via `BulkCheckPermission`.
  - Done: in-crate `audit/` emitter + envelope; per-RPC audit from agent `Bridge::dispatch` when a non-noop emitter is configured; gateway decision-site `AuditEnvelope` with `trace_id`, `rules_fired`, `rewrites`, and `stream_consumer` (§Audit example — no `tenant` field per §Decisions).
  - Pending: Tier 1 policies, SpiceDB authz, discovery shaping.
- **Phase 2** *(partial)* — `message/stream` and `tasks/resubscribe` over JetStream consumers. Task-lifecycle audit. Tier 2 CEL.
  - Done: `message/stream` and `tasks/resubscribe` end-to-end, ephemeral pull consumers with `inactive_threshold`, resubscribe RPC + JetStream replay at `last_seq + 1`; per-state-transition `TaskLifecycleEnvelope` emitted to `{prefix}.audit.lifecycle` when streamed `TaskStatusUpdateEvent.status.state` changes.
  - Pending: equivalent lifecycle coverage for unary `message/send` task evolution — tracked as product-scope in [`A2A_PENDING_DECISION.md`](./A2A_PENDING_DECISION.md) §Unary RPC vs streamed lifecycle envelopes (callers observe post-reply transitions today only by attaching via `message/stream` / `tasks/resubscribe` or to the task event subject); Tier 2 CEL.
- **Phase 3** *(partial)* — `tasks/pushNotificationConfig/*` with NATS-subject and JetStream-stream targets; HTTPS webhook target as a special case. Tier 3 WASM redaction over `parts[*]`.
  - Done: `tasks/pushNotificationConfig/*` CRUD round-trips through the handler; in-crate `push/` dispatcher; terminal-status delivery wired in the `message/stream` event pump via `CompositePushDispatcher` covering HTTP(S) (`http(s)://…`), core NATS (`subject:{…}`), and JetStream ack-publish (`jetstream:{…}`; caller-provisioned stream covering the subject); bounded in-process retries with backoff (**`constants::HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS`**) — status- and transport-aware for HTTPS; same attempt cap / exponential backoff for NATS core and JetStream publish paths; `pushNotificationAuthenticationInfo` mapping via `push/authentication_header.rs` (Bearer / Basic / jwt → Bearer); **`A2A_PUSH_DLQ`** JetStream asset created alongside **`A2A_EVENTS`** (`provision_streams` / `nats/subjects/stream.rs`); **`push::dlq`** publishes JSON envelopes on terminal dispatch failure (default DLQ **`caller_id`** segment **`_`**).
  - Pending: optional gateway-side DLQ mirroring; Tier 3 WASM redaction **engine landed** in `a2a-redaction` (Wasmtime + skill-id dispatch) — gateway request-path call site remains. Exactly-once opt-in **landed** (`DeliverySemantics`, `PushIdempotencyKey`, `IdempotencyKeyHeader`; sketch **[`docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md`](./docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md)**). Digest webhook auth deferred per §Decisions.
- **Phase 4** *(partial)* — `a2a-bridge` HTTPS↔NATS runtime landed: axum HTTPS, auth-callout client, NATS publish/consume, SSE↔JetStream framing, outbound HTTPS path, trait-mocked tests (see **`docs/A2A_BRIDGE_SKETCH.md`**). Federated discovery **`SpiceDbImportGate`** (`CheckBulkPermissions` + ZedToken cache, env-gated deny-only default) and import-gate trait landed in `a2a-nats::catalog::import_gate` (**[`docs/A2A_FEDERATED_DISCOVERY_SKETCH.md`](./docs/A2A_FEDERATED_DISCOVERY_SKETCH.md)**). Pending: production wiring against a deployed auth-callout, cross-binding collaboration tests against a live NATS+gateway, operator-signed Account export contract for `a2a.discover.>`.

## Existing Code to Lean On

- `trogon-gateway` — recently grew Slack Socket Mode (commit `d0cf94481`); same crate is the natural home for the HTTPS↔NATS A2A bridge.
- `trogon-nats` — auth config and connection management, including JetStream helpers expanded in `094015b37`.
- `rsworkspace/crates/mcp-nats` — JSON-RPC-over-NATS conventions to mirror (subject parsing, peer id, reply inbox model).
- `mcp-gateway` / `trogon-policy` (from `MCP_GATEWAY_PLAN.md`) — the engine and bundle distribution we reuse; A2A is a bundle on top, not a fork.

New crates this plan adds:

- `a2a-nats` *(shipped)* — protocol binding (encoding, subject layout, task event helpers, AgentCard types) plus in-crate `catalog/`, `push/`, and `audit/` modules. Peer of `mcp-nats`.
- `a2a-nats-agent` *(shipped, not originally in plan)* — daemon shell wrapping a user-supplied `A2aHandler`. Equivalent of an MCP "stdio server" entry point.
- `a2a-nats-server` *(shipped, narrower than `a2a-bridge`)* — axum HTTP/SSE adapter for the local **`a2a_nats::Client`**; README documents gateway env knobs. Does not bridge foreign HTTPS A2A clients end-to-end.
- `a2a-nats-stdio` *(shipped, not originally in plan)* — line-delimited JSON-RPC stdio adapter.
- `a2a-nats-discovery` *(shipped, not originally in plan)* — KV registrar binary; provisions `A2A_AGENT_CARDS` and runs `DiscoverService` on `{prefix}.discover.*` plus `CatalogRegistrarService` on `{prefix}.catalog.register.*`.
- `a2a-gateway` *(partial)* — optional queue-group subscriber on **`{prefix}.gateway.>`**; forwards JSON-RPC payloads to **`{prefix}.agent.{agent_id}.{method}`** with the caller reply inbox, deadline-bounded for `message.send` (`A2A_GATEWAY_UNARY_DEADLINE_SECS`). Policy stack in `src/policy/`: Tier-1 SpiceDB (`A2A_GATEWAY_TIER1_SPICEDB_ENABLED`), Tier-2 predicate seam when `A2A_GATEWAY_POLICY_BUNDLE_DIR` is set (`NoopTier2Evaluator` until CEL→WASM), Tier-3 Wasmtime preload via `A2A_GATEWAY_POLICY_SKILLS`; decision-site `AuditEnvelope` JSON published on `{prefix}.audit.{ok,err}.{method}` when `A2A_GATEWAY_AUDIT_PUBLISH` is on (`trace_id`, `rules_fired`, `rewrites`, `stream_consumer`, `zed_token_snapshot`). Caller attribution rides NATS header `X-A2a-Caller-Id` (HTTPS `x-a2a-caller-id` mapped by `a2a-bridge`). Auth-callout client, authoritative Tier-2 CEL evaluator, and Tier-3 redaction call site remain to be built.
- `a2a-pack` *(partial)* — AgentCard JSON Schema landed (`agent_card_schema::validate_agent_card_value`, enforced by `KvCatalogStore`). Other modules (tuples, audit envelope extensions, redaction rules, rate-limit profiles) still placeholders.
- `a2a-bridge` *(partial)* — HTTPS↔NATS sidecar runtime: axum HTTPS surface, auth-callout client, async-nats publish/consume, SSE↔JetStream framing, outbound HTTPS for NATS-in/HTTPS-out, trait-mocked tests. Production-mode wiring against a deployed auth-callout still future.
- `a2a-auth-callout` *(partial)* — verifier surface: OIDC JWKS, mTLS x509 chain, HMAC-SHA256 API-key (transitional); Account-bound User JWT mint. Deployed subscriber on `$SYS.REQ.USER.AUTH` is operator work.
- `a2a-redaction` *(partial)* — Wasmtime engine + module loader + skill-id dispatch for Tier 3 part-level redaction. Gateway request-path call site still future.
