# A2A over NATS — Architecture

A NATS-native binding for the [Agent2Agent (A2A) protocol](https://a2aproject.github.io/A2A/) — agent discovery, task lifecycle, streaming updates, and push notifications — running on NATS subjects and JetStream instead of HTTP/SSE/webhooks.

## Intent

A2A is JSON-RPC 2.0 over HTTPS with SSE for streaming and HTTP webhooks for push. The protocol design is transport-agnostic at the semantic layer: AgentCards, tasks, messages, artifacts, and the streaming task lifecycle are the contract. The transport is replaceable.

Provide a transport binding where:

- **Discovery** — AgentCards are published to and resolved from a NATS-backed catalog instead of `/.well-known/agent.json`.
- **Unary methods** — `message/send`, `tasks/get`, `tasks/cancel`, `tasks/pushNotificationConfig/*` map to NATS request/reply on per-agent subjects.
- **Streaming** — `message/stream` and `tasks/resubscribe` map to JetStream consumers on per-task event subjects; clients re-attach by replaying from a sequence.
- **Push notifications** — replaced by durable JetStream subscriptions on caller-owned subjects; the HTTP webhook becomes a NATS publish.
- **Identity** — A2A's "declared auth schemes in AgentCard" becomes a NATS auth callout that mints a scoped user JWT; the AgentCard advertises NATS-native schemes.

The result is opaque agent-to-agent collaboration with the same semantics as HTTP A2A, but with NATS' operational model: queue-group horizontal scaling, JetStream-backed task event retention, KV-backed discovery, and replay/resubscribe as a first-class primitive instead of a webhook bolt-on.

Navigation: [`./README.md`](../README.md) · env reference [Runtime env](../reference/runtime-env.md) · streaming back-pressure [Streaming back-pressure](../how-to/operators/streaming-backpressure.md).

## Protocol Mapping

### Tenancy model

**Tenant = NATS Account.** Each tenant runs in a dedicated NATS Account; subjects below have **no `{tenant}` segment** because Account membership is the tenancy boundary. Cross-tenant traffic requires explicit operator-signed Account exports/imports — federation is opt-in. JetStream streams and KV buckets use the same names across Accounts (`A2A_EVENTS`, `A2A_AGENT_CARDS`); the Account namespace disambiguates.

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

A2A's `/.well-known/agent.json` is replaced by a NATS KV bucket `A2A_AGENT_CARDS` keyed by `{agent_id}`. Two access patterns:

- **Pull** — clients fetch via KV `get` with optional revision watch (`A2aClient::watch_agent_card`).
- **Service-style** — `a2a.discover.{agent_id}` is a request/reply endpoint that returns the AgentCard, suitable for clients that don't speak KV.

#### Why KV, not scatter-gather

Scatter-gather (the NATS Services `$SRV.PING` / `$SRV.INFO` pattern) is the reflex NATS-idiomatic answer for "who's online?" and was considered. We chose KV instead. The reasoning, recorded so it doesn't get re-litigated in review:

- **AgentCards are semi-static config, not runtime state.** Skills, schemas, examples, auth schemes change on deploy, not constantly. KV is shaped for write-rarely, read-often, watch-for-changes.
- **KV watchers beat re-scattering.** Discovery clients `watch` the bucket once and get push updates on change. Scatter-gather forces a fresh fan-out on every query.
- **Fan-out cost scales with fleet size.** N agents × every discovery = N replies per query, plus a dedupe layer for queue-group replicas. KV `get` is O(1).
- **Authz shaping caches better against a stable set.** `BulkCheckPermission` on a known agent id list reuses ZedToken cache; on a dynamic gather result it doesn't, and races against agents joining/leaving mid-gather.
- **Semantic parity with HTTPS A2A.** The spec's discovery model is *pull on demand* (`/.well-known/agent.json`). KV `get` matches one-to-one; scatter-gather is a behavior delta the HTTPS bridge would have to paper over.
- **Liveness from PING is theatre on the discovery path.** "Agent responded to PING" ≠ "agent will accept the next message". The request/reply on `a2a.agent.{id}.message.send` is the real liveness signal — and it's the call the client was about to make anyway.

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
client ──PUB──▶ a2a.gateway.{agent_id}.message.send  (JSON-RPC envelope, reply_to = _INBOX.x)
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
  2. ensures the Account's `A2A_EVENTS` JetStream stream with subject filter `a2a.task.>` exists,
  3. forwards the request to the agent on `a2a.agent.{agent_id}.message.stream` with `events_subject = a2a.task.{task_id}.events`,
  4. creates an ephemeral pull consumer for the caller bound to that subject filter, delivering to a caller inbox,
  5. emits an immediate JSON-RPC ack containing `{task_id, stream_inbox, last_seq: 0}`.

The agent publishes each `Task` / `TaskStatusUpdateEvent` / `TaskArtifactUpdateEvent` to `a2a.task.{task_id}.events`. The gateway consumer rewrites/redacts in flight and forwards to the caller inbox.

`tasks/resubscribe` becomes a new ephemeral consumer on the same subject starting from `OptStartSeq = last_seq + 1` — the durable equivalent of SSE reconnect, but lossless within JetStream retention.

### Push notifications

`tasks/pushNotificationConfig/set` registers a delivery target. The NATS binding accepts:

- **NATS subject** — `subject:a2a.push.caller-42.task-9` — gateway publishes the same envelope SSE would carry.
- **JetStream stream + subject** — `jetstream:…`, but the caller pre-creates a durable stream for at-least-once delivery across disconnects.
- **HTTPS webhook** — preserved for cross-binding interop; the gateway acts as a webhook client.

Push deliveries use the same JSON envelope as the SSE stream; payload semantics are unchanged. Auth on the push target follows A2A's existing `pushNotificationAuthenticationInfo` model (bearer / signed JWT). For NATS targets, identity is the gateway's own NATS user.

### Method → subject table

| JSON-RPC method                                    | Subject (client → gateway)                                            | Direction       |
|----------------------------------------------------|------------------------------------------------------------------------|-----------------|
| `message/send`                                     | `a2a.gateway.{agent_id}.message.send`                                  | req/reply       |
| `message/stream`                                   | `a2a.gateway.{agent_id}.message.stream`                                | req → stream    |
| `tasks/get`                                        | `a2a.gateway.{agent_id}.tasks.get`                                     | req/reply       |
| `tasks/cancel`                                     | `a2a.gateway.{agent_id}.tasks.cancel`                                  | req/reply       |
| `tasks/resubscribe`                                | `a2a.gateway.{agent_id}.tasks.resubscribe`                             | req → stream    |
| `tasks/pushNotificationConfig/set\|get\|list\|delete` | `a2a.gateway.{agent_id}.tasks.pushNotificationConfig.{op}`             | req/reply       |
| `agent/getAuthenticatedExtendedCard`               | `a2a.gateway.{agent_id}.agent.getAuthenticatedExtendedCard`            | req/reply       |

## Architecture

### Components

All components run **inside the tenant's NATS Account**; cross-Account traffic is gated by operator-signed exports/imports.

1. **Auth callout service** — terminates external OIDC / mTLS / API keys and mints a User JWT bound to the tenant's Account. Subject ACL inside the Account confines the caller to `a2a.gateway.>` and an `_INBOX.{caller_id}.>` reply space, plus consumer ACL on caller-owned push subjects.
2. **A2A gateway service** — queue-group subscriber on `a2a.gateway.>` inside the Account; stateless per-message except for streaming where it owns the consumer-to-caller pipe.
3. **Discovery / catalog service** — owns the Account's `A2A_AGENT_CARDS` KV bucket; serves `a2a.discover.>` requests. Registrars publish AgentCards; gateway validates against AgentCard JSON Schema before accepting. Federated discovery (across Accounts) requires explicit Account exports of `a2a.discover.>`.
4. **Task stream manager** — ensures the Account's `A2A_EVENTS` JetStream stream exists with subject filter `a2a.task.>`, configures retention (default `interest` with `max_age = 24h`, per-Account override), provisions consumers.
5. **Policy engine** — Tier-1 SpiceDB + declarative, Tier-2 CEL, Tier-3 WASM redaction. A2A resource-tuple derivation table layered as a bundle. Policies evaluate inside the Account; cross-Account SpiceDB principals carry Account identity.
6. **Audit emitter** — `a2a.audit.{outcome}.{method}` inside the Account (Account namespace = tenant attribution).
7. **HTTP↔NATS bridge (`a2a-bridge`)** — accepts vanilla A2A HTTPS clients on one side, mints an Account-bound NATS User JWT, publishes inside the caller's tenant Account on the other. Lets external agents that only speak HTTP A2A participate without code change.

### Backend agents

Agents subscribe to `a2a.agent.{agent_id}.>` as a queue group. An A2A SDK adapter handles:

- JSON-RPC envelope decode,
- task state machine bookkeeping,
- publishing task events to the per-task subject the gateway hands them.

### Streaming flow

```
caller ──PUB──▶ a2a.gateway.{agent}.message.stream
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
                  publish events ──PUB──▶ a2a.task.{task_id}.events
                                        │
                ┌───────────────────────┘
                ▼
          Gateway consumer ──rewrite / shape / audit──▶ caller stream inbox
```

Caller disconnect: ephemeral consumer dies; on `tasks/resubscribe` the gateway recreates it at `last_seq + 1`. Lossless within retention window. Push-notification targets see the same events independently via their own durable consumer.

## Auth Model

**Tenant boundary = NATS Account.** Each tenant runs in a dedicated Account; auth produces an Account-bound User JWT, and subject ACLs are written *inside* that Account.

AgentCards declare `auth_schemes` per transport. NATS-bound schemes:

- **`nats-callout`** — external OIDC / mTLS / API key terminates at a callout service that mints a User JWT bound to the tenant's Account. Subject ACL inside the Account bounds the caller to `a2a.gateway.{agent_id}.>` and an `_INBOX.{caller_id}.>` reply space.
- **`nkey`** — direct NKey-based User inside the tenant's Account for service-to-service agents.
- **`jwt-bearer`** — passthrough JWT carried in the JSON-RPC envelope `metadata`, validated by the gateway against AgentCard-declared issuers; Account membership still required at the connection layer.

Cross-binding: when the HTTPS↔NATS bridge accepts an HTTPS A2A client, it terminates HTTP auth and re-mints a NATS User JWT in the caller's tenant Account. AgentCard's `security` block stays HTTPS-shaped on the outside; NATS-bound peers see the NATS-native schemes. Cross-tenant access is only possible through operator-signed Account exports — off by default.

## Policy Engine

Three tiers run in the gateway: Tier-1 SpiceDB + declarative authz, Tier-2 CEL, Tier-3 WASM redaction over `Message.parts[*]` and `Artifact.parts[*]`. The A2A bundle ships:

- Resource-tuple derivation for A2A methods (table below).
- Catalog shaping rules — filter AgentCards in the `a2a.discover` response by caller permission (`BulkCheckPermission`-driven, via `LiveAgentViewGate`).
- Schema-driven redaction over `parts[*]`, indexed by AgentCard skill definitions.
- Per-task quota and rate-limit policies (`a2a_pack::rate_limit::default_rate_limit_profiles`).
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

`a2a.audit.{outcome}.{method}` envelopes carry decision-site attribution:

```json
{
  "ts": "2026-05-22T10:00:00Z",
  "trace_id": "...",
  "caller": { "sub": "user:alice", "via": "oidc:google" },
  "method": "message/stream",
  "agent_id": "support-bot",
  "task_id": "t_abc123",
  "task_state_before": "submitted",
  "task_state_after": "working",
  "tier1_decision": "allow",
  "tier3_decision": "allow",
  "rules_fired": ["a2a-invoke-stream-authz", "redact-message-parts"],
  "rewrites": [{ "path": "$.params.message.parts[0].text", "op": "classify:pii.email" }],
  "stream_consumer": "A2A_EVENTS/c_x9",
  "latency_us": 1820
}
```

Per-task lifecycle is recoverable from audit alone: every state transition emits an envelope (`TaskLifecycleEnvelope` on `{prefix}.audit.lifecycle`). SIEM consumers subscribe via durable JetStream consumer.

## Bundles

Signed bundle packaging, distribution, and hot-swap via `a2a-pack`. First-party content:

- Resource-tuple table.
- Catalog shaping for AgentCard discovery.
- AgentCard JSON Schema validation (registration-time guard, enforced by `KvCatalogStore` on KV get/put).
- Schema-driven redaction over `parts[*]` keyed by skill id.
- Task lifecycle audit shape.
- Default rate-limit profile per skill kind.
- Clock-aware Tier-1 declarative bundles (per-method allowlist, per-agent allowlist, `time_of_day`).
- Tier-3 redaction skill catalog (`pii-regex-redactor`, `secrets-redactor`, `json-path-sanitizer`).

Tenants extend by overlaying bundles.

## Interop Bridge

`a2a-bridge` makes the NATS binding interoperable with the HTTPS A2A ecosystem in both directions:

- **HTTPS in, NATS out** — terminates standard A2A HTTPS, translates JSON-RPC to NATS publish, streams SSE back from the JetStream consumer.
- **NATS in, HTTPS out** — registers external HTTPS agents into the NATS catalog as proxied AgentCards; gateway forwards to the HTTPS endpoint and adapts SSE → JetStream events.

Auth is re-minted at the bridge in both directions. AgentCards declare both transports so polyglot clients pick.

## Decisions

- **Tenancy = NATS Account per tenant.** Cross-Account traffic requires explicit operator-signed exports/imports; federation is opt-in. Subject table carries no `{tenant}` segment; stream/KV names are reused across Accounts.
- **Stream topology = shared `A2A_EVENTS` per Account, subject-filtered `a2a.task.>`**. Per-task streams rejected — JS metadata cost dominates at fleet scale.
- **Retention default = 24h baseline, per-Account override**. JetStream `max_age = 24h`.
- **Push delivery semantics = at-least-once default**, exactly-once as opt-in flag on `PushNotificationConfig`.
- **AgentCard write path = central registrar (`a2a-nats-discovery`) validates against JSON-Schema, then writes KV.** Schema lives in `a2a-pack`; gateway re-validates on read.
- **Streaming back-pressure = gateway pull consumer with flow control + stream policy `retention=interest, discard=old`** (egress: `A2A_GATEWAY_EVENTS_PULL=on`; ingress: `A2A_GATEWAY_STREAMING_INGRESS=on`). Agents never block on publish.
- **`message/send` max blocking window = 30s.** Longer work forced onto `message/stream`. Gateway enforces this via `A2A_GATEWAY_UNARY_DEADLINE_SECS` (default `DEFAULT_OPERATION_TIMEOUT`); overrun replies JSON-RPC `-32800`.
- **Bridge identity = auth-callout mints per-request User inside the caller's tenant Account.** Single-bridge-User rejected to preserve caller attribution in audit.
- **Federated discovery = off by default; opt-in via operator-signed exports of `a2a.discover.>`**; SpiceDB gates the import side.
- **Cross-process push DLQ = per-Account `A2A_PUSH_DLQ` JetStream stream** at `a2a.push.dlq.{caller_id}.{task_id}`.
- **Digest webhook auth = deferred.** Currently rejected with a clear error.
- **NATS-push auth = gateway NATS User + subject ACL on `a2a.push.>`** (gateway publish) and `a2a.push.{caller_id}.>` (caller read).
- **Audit envelope = no `tenant` field** (Account namespace carries the attribution).
- **Auth callout = NATS subscriber on `$SYS.REQ.USER.AUTH`**; OIDC primary, mTLS for service-to-service, API keys transitional. Mints Account-bound User JWT.
- **Policy substrate = single Wasmtime runtime in the gateway** hosting Tier 2 (CEL compiled to WASM at bundle build) and Tier 3 (WASM redaction).
- **SpiceDB = org-standard cluster, gateway holds the client.** Resource tuples per §Policy Engine.
- **`a2a-bridge` = HTTPS sidecar; per-request re-mint into caller's Account; SSE↔JetStream consumer mapping** on `a2a.task.{task_id}.events.>`.
