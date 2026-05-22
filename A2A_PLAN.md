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

## Implementation Status

Snapshot of what is in-tree vs. what this plan still describes as future work.

### Shipped crates

- `rsworkspace/crates/a2a-nats` — protocol binding: subjects, JetStream stream/consumer/provision, JSON-RPC envelope, `Bridge` (agent runtime) and `Client`.
- `rsworkspace/crates/a2a-nats-agent` — daemon shell that drives a user-supplied `A2aHandler` against the `Bridge`. Ships a `NoopHandler` and an `examples/echo.rs` reference handler.
- `rsworkspace/crates/a2a-nats-server` — axum HTTP/SSE front-end that adapts JSON-RPC over HTTP to the NATS client. Not yet an A2A↔HTTPS interop bridge — it speaks to its own agent only.
- `rsworkspace/crates/a2a-nats-stdio` — line-delimited JSON-RPC stdio bridge for tools that cannot embed NATS.

### Working surface (Phase 2 partial)

- Unary methods: `message/send`, `tasks/get`, `tasks/list`, `tasks/cancel`, `tasks/pushNotificationConfig/{set,get,list,delete}`, `agent/getAuthenticatedExtendedCard`. Each routes through `a2a.agent.{agent_id}.{method}` to an `A2aHandler` impl.
- Streaming: `message/stream` (bootstrap reply + JetStream pump) and `tasks/resubscribe` (RPC for snapshot + JetStream consumer at `last_seq + 1`). Backed by a single `A2A_EVENTS` stream filtered to `a2a.task.*.events.*`.
- Backpressure: `Config::max_concurrent_client_tasks` enforced via a Semaphore in the `Bridge` dispatch loop.
- Consumer hygiene: ephemeral consumers carry `inactive_threshold = 5m` so client crashes don't leak server state.

### Intentional simplifications vs. the design below

- **No gateway / tenant axis yet.** Live subjects are `a2a.agent.{agent_id}.{method}` and `a2a.task.{task_id}.events.{req_id}` — the `a2a.gateway.{tenant}.>` namespace and the `{tenant}` segment on task subjects are not yet wired. Adding the tenant axis is a rename + an upstream gateway service; the handler trait is unaffected.
- **Single stream `A2A_EVENTS`** instead of per-tenant `A2A_TASKS_{tenant}`.
- **No auth callout, no policy engine, no SpiceDB authz, no audit emitter.** Anything that reaches the NATS subject is dispatched. Auth is delegated to the underlying NATS account / NKey.
- **Push notification delivery is not implemented.** The `tasks/pushNotificationConfig/*` methods round-trip config through the handler, but no component reads that config and POSTs/publishes when an event fires.
- **AgentCard generation is the handler's responsibility.** No KV catalog, no JSON-Schema validation, no `a2a.discover.{tenant}.>` service.
- **No HTTPS↔NATS interop bridge** for foreign A2A clients.

### Crates this plan still calls for

- `a2a-gateway` — queue-group gateway sitting between clients and agents (auth, policy, audit, stream/consumer provisioning).
- `a2a-bridge` — HTTPS↔NATS interop sidecar in both directions.
- `a2a-pack` — first-party policy bundle (resource tuples, catalog shaping, redaction, audit envelope).
- A discovery service that owns the `A2A_AGENTCARDS_{tenant}` KV bucket.

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

### Subject layout

Two namespaces separated by NATS authorization:

| Subject                                         | Role                                                          | Who publishes              |
|-------------------------------------------------|---------------------------------------------------------------|----------------------------|
| `a2a.gateway.{tenant}.{agent_id}.{method}`      | Client → gateway request inbox                                | Clients                    |
| `a2a.agent.{agent_id}.{method}`                 | Gateway → agent backend                                       | Gateway                    |
| `a2a.task.{tenant}.{task_id}.events`            | Per-task event stream (JetStream)                             | Agent (via gateway)        |
| `a2a.push.{tenant}.{caller_id}.{task_id}`       | Caller-owned push delivery subject                            | Gateway                    |
| `a2a.discover.{tenant}.>`                       | AgentCard catalog (KV-backed; also a service subject)         | Registrars                 |

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

1. **Auth callout service** — same shape as in MCP plan; mints scoped JWT with subject ACL on `a2a.gateway.{tenant}.>` and consumer ACL on caller-owned push subjects.
2. **A2A gateway service** — queue-group subscriber on `a2a.gateway.>`; stateless per-message except for streaming where it owns the consumer-to-caller pipe.
3. **Discovery / catalog service** — owns `A2A_AGENTCARDS_{tenant}` KV bucket; serves `a2a.discover.{tenant}.>` requests. Registrars publish AgentCards; gateway validates against AgentCard JSON Schema before accepting.
4. **Task stream manager** — ensures `A2A_TASKS_{tenant}` JetStream stream exists, configures retention (default `interest` with `max_age = 24h`, per-tenant override), provisions consumers.
5. **Policy engine** — reused from MCP plan; A2A resource-tuple derivation table layered as a bundle.
6. **Audit emitter** — `a2a.audit.{tenant}.{outcome}.{method}`.
7. **HTTP↔NATS bridge (optional, sidecar)** — accepts vanilla A2A HTTPS clients on one side, publishes NATS on the other. Lets external agents that only speak HTTP A2A participate without code change.

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

AgentCards declare `auth_schemes` per transport. NATS-bound schemes:

- **`nats-callout`** — external OIDC / mTLS / API key terminates at a callout service that mints a scoped user JWT. Subject ACL bounds the caller to `a2a.gateway.{tenant}.{agent_id}.>` and an `_INBOX.{caller_id}.>` reply space.
- **`nkey`** — direct NKey-based account for service-to-service agents.
- **`jwt-bearer`** — passthrough JWT carried in the JSON-RPC envelope `metadata`, validated by the gateway against AgentCard-declared issuers.

Cross-binding: when the HTTPS↔NATS bridge accepts an HTTPS A2A client, it terminates HTTP auth and re-mints a NATS JWT scoped to the caller's identity. AgentCard's `security` block stays HTTPS-shaped on the outside; NATS-bound peers see the NATS-native schemes.

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

`BulkCheckPermission` is used for catalog shaping on the discovery endpoint. ZedToken consistency scoped per-session.

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

## Open Questions

1. **Stream-per-task vs. shared stream with subject filter** — one JetStream stream per tenant filtered by `a2a.task.{tenant}.>` is operationally cleaner than one stream per task. Retention and quota then live at the tenant level, which is what the audit team will want anyway. Confirm at scale.
2. **Retention default for task events** — A2A doesn't specify SSE history beyond resubscribe. We pick a default (24h?) and let tenants override. Too short kills resubscribe; too long inflates storage.
3. **Push delivery semantics** — at-least-once via durable consumer is the obvious default. Exactly-once is achievable with JetStream double-ack but adds a hop; probably not worth it until someone asks.
4. **AgentCard registry write path** — only the agent owner can publish its card (KV key per agent, with NKey-bound write ACL) versus a central catalog service that validates and republishes. The latter gives schema enforcement; the former gives autonomy.
5. **Streaming back-pressure** — JetStream consumer flow control vs. blocking publishes on the agent side. Default to flow-controlled pull consumer for the gateway, but agents publishing fast enough to fill a stream is a real failure mode worth designing for.
6. **`message/send` task lifecycle** — A2A allows `message/send` to return either a complete `Task` or be polled later. Over NATS this is just request/reply latency; the question is whether to enforce a max blocking window and force long-running work onto `message/stream`.
7. **Bridge identity model** — does the HTTPS↔NATS bridge present as a single NATS identity (its own) with the caller's sub in `metadata`, or does it impersonate per request via a delegated auth callout?
8. **Discovery scope** — tenant-scoped catalog only, or also a cross-tenant federated catalog with SpiceDB-gated visibility?

## Phased Delivery

Aligned with the MCP plan's phases so they share infrastructure.

- **Phase 0** *(partial)* — auth callout + subject ACL + AgentCard KV bucket + unary `message/send` and `tasks/get` end-to-end. No streaming, no push, no policy. Prove the perimeter and the catalog.
  - Done: unary `message/send`, `tasks/get`, and the rest of the unary surface.
  - Pending: auth callout, subject ACL, AgentCard KV bucket / catalog service.
- **Phase 1** *(not started)* — Tier 1 policies, SpiceDB authz on `message/send` and `tasks/*`, audit to JetStream. Discovery shaping via `BulkCheckPermission`.
- **Phase 2** *(partial)* — `message/stream` and `tasks/resubscribe` over JetStream consumers. Task-lifecycle audit. Tier 2 CEL.
  - Done: `message/stream` and `tasks/resubscribe` end-to-end, ephemeral pull consumers with `inactive_threshold`, resubscribe RPC + JetStream replay at `last_seq + 1`.
  - Pending: task-lifecycle audit envelopes, Tier 2 CEL.
- **Phase 3** *(not started)* — `tasks/pushNotificationConfig/*` with NATS-subject and JetStream-stream targets; HTTPS webhook target as a special case. Tier 3 WASM redaction over `parts[*]`.
  - Done: `tasks/pushNotificationConfig/*` CRUD round-trips through the handler.
  - Pending: delivery — nothing currently reads the config and dispatches notifications. Tier 3 WASM redaction.
- **Phase 4** *(not started)* — HTTPS↔NATS bridge, federated catalog, cross-binding agent collaboration.

## Existing Code to Lean On

- `trogon-gateway` — recently grew Slack Socket Mode (commit `d0cf94481`); same crate is the natural home for the HTTPS↔NATS A2A bridge.
- `trogon-nats` — auth config and connection management, including JetStream helpers expanded in `094015b37`.
- `rsworkspace/crates/mcp-nats` — JSON-RPC-over-NATS conventions to mirror (subject parsing, peer id, reply inbox model).
- `mcp-gateway` / `trogon-policy` (from `MCP_GATEWAY_PLAN.md`) — the engine and bundle distribution we reuse; A2A is a bundle on top, not a fork.

New crates this plan adds:

- `a2a-nats` *(shipped)* — protocol binding (encoding, subject layout, task event helpers, AgentCard types). Peer of `mcp-nats`.
- `a2a-nats-agent` *(shipped, not originally in plan)* — daemon shell wrapping a user-supplied `A2aHandler`. Equivalent of an MCP "stdio server" entry point.
- `a2a-nats-server` *(shipped, narrower than `a2a-bridge`)* — axum HTTP/SSE adapter for the local agent. A starting point for the planned interop bridge but does not yet bridge foreign A2A HTTPS clients.
- `a2a-nats-stdio` *(shipped, not originally in plan)* — line-delimited JSON-RPC stdio adapter.
- `a2a-gateway` *(not yet)* — queue-group gateway service. Peer of `mcp-gateway`.
- `a2a-bridge` *(not yet)* — HTTPS↔NATS interop sidecar in both directions.
- `a2a-pack` *(not yet)* — first-party policy bundle (resource tuples, shaping, redaction, audit envelope).
