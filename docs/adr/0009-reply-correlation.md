# ADR 0009: Reply Correlation Mechanism

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-28) |
| **Date** | 2026-05-28 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | Reply correlation mechanism; Phase 2 gateway implementation in `trogon-mcp-gateway` |
| **Related** | `docs/identity/reply-correlation.md`, `docs/identity/reference-reply-inboxes.md`, `docs/identity/reference-queue-groups.md` |

## Context

The Trogon MCP gateway is a **queue-group worker**: NATS delivers each ingress message on `{prefix}.gateway.request.>` to any healthy replica subscribed under the `mcp-gateway` queue group. MCP clients, however, expect **request/reply semantics** — a JSON-RPC call with an `id` must receive exactly one matching response on the reply subject they attached to the publish.

Before changing the reply path, operators and implementers need a durable decision on **how the gateway correlates backend replies back to the client's inbox** when load balancing spreads ingress across replicas.

**Phase 1 behaviour (shipped today):** The queue-group member that consumes an ingress message performs policy, calls the backend via inline `request_with_headers`, and publishes the backend payload to the **same** `Message.reply` the client supplied. Reply routing is implicit in the NATS client's pending-request table for the duration of one hop. No explicit correlation map exists in gateway code.

**Phase 2 target:** The gateway **terminates** the client's reply subject. It mints a per-instance backend inbox, stores the original client reply in process memory, and subscribes directly to `_INBOX.gateway.{instance_id}.>` on the owning replica only.

**Design context:** The full failure-mode analysis, dedup contract, idempotency promises, and observability plan live in `docs/identity/reply-correlation.md`. Inbox token shapes, subscription topology, deadline propagation, and orphaned-reply handling are specified in `docs/identity/reference-reply-inboxes.md`. Queue-group vs direct-subscribe rules are in `docs/identity/reference-queue-groups.md` (Wire-Format Pin 3).

**What breaks if this stays undecided:**

- Phase 2 gateway work cannot land a consistent reply path — implementers may fork between NATS server transforms and in-memory maps.
- HA semantics remain ambiguous: operators cannot document client retry expectations or replica-death behaviour.
- Wire-Format Pin 2 (inbox naming) and Pin 3 (queue groups) lack an authoritative correlation backing story.
- Audit and tracing cannot attach `mcp-producer-replica-id` and `reply_orphaned` events to a single architectural model.

### The queue-group problem

Queue-group load balancing is correct for throughput and failover on **new** messages. It is insufficient for **in-flight** reply routing unless an explicit correlation layer exists. Three failure modes motivate this decision (detailed in `docs/identity/reply-correlation.md` §2):

| Mode | Root cause | Client symptom |
|------|------------|----------------|
| (a) Replica death | Process-local correlation lost mid-request | NATS request timeout |
| (b) Reply drop | Best-effort core NATS publish lost after gateway success | Timeout despite backend completion |
| (c) Retry overlap | No dedup; independent queue delivery of retries | Duplicate responses / double side effects |

This ADR decides **where correlation state lives** for Phase 1 wire format and Phase 2 implementation. Cross-replica dedup (JetStream KV) is a complementary Phase 2b concern documented in the spec; it does not replace the in-memory inbox map.

### NATS subject-transform alternative

NATS server configuration supports **subject mapping / transform** rules that can rewrite publish subjects while optionally preserving the original `reply-to` header on the transformed message. In theory, a transform could rewrite ingress to backend subjects and pass the client's `_INBOX.client.*` through to the backend unchanged, letting the backend reply directly to the client without gateway termination.

That path was evaluated against the gateway's requirements:

| Requirement | Subject-transform | In-memory termination |
|-------------|-------------------|----------------------|
| Per-message policy before backend forward | Hard — transform runs before gateway worker sees the message | Natural — gateway evaluates CEL/SpiceDB on ingress |
| Operator control without NATS server churn | Requires server config change per deployment | Gateway config only |
| Observable in-flight state | Opaque inside NATS core | Bounded `HashMap` gauge per replica |
| Wire-Format Pin 2 inbox shapes | Bypasses `_INBOX.gateway.{instance_id}.{nuid}` | Matches pinned contract |
| Latency on hot path | Extra server-side transform hop | Map insert + direct publish (no KV round-trip for correlation) |

Subject-transform with inbox preservation is viable for simpler proxies but conflicts with the gateway's role as a policy and audit termination point. It also externalizes correlation to NATS server ops, making per-tenant or per-method reply policy harder to enforce consistently.

## Decision

**Gateway terminates the client's `reply-to` and issues its own per-instance inbox to the backend (`_INBOX.gateway.{instance_id}.{nuid}`). The original client `reply-to` is held in an in-memory bounded `HashMap<Nuid, ClientReplyTo>`. Subject-transform alternatives are rejected for Phase 1.**

Concretely:

1. **Ingress:** Client publishes to `{prefix}.gateway.request.{server_id}.{method}` with `Message.reply` set to `_INBOX.client.{nuid_c}` (or another client-chosen subject). Queue group `mcp-gateway` delivers to one replica.

2. **Termination:** That replica mints `{nuid_g}`, inserts `HashMap[{nuid_g}] = ClientReplyTo { original_reply_to, request_ctx, span_ctx, deadline }`, and publishes to the backend with `reply-to: _INBOX.gateway.{instance_id}.{nuid_g}` and header `mcp-instance-id: {instance_id}`.

3. **Backend reply:** Backend publishes to `_INBOX.gateway.{instance_id}.{nuid_g}`. Only the owning replica's direct subscription receives it.

4. **Client reply:** Owning replica looks up `{nuid_g}`, publishes JSON-RPC result or error to `original_reply_to`, removes the map entry, and emits audit/trace with `mcp-producer-replica-id`.

5. **No sticky routing:** Any replica may serve any **new** ingress message. Affinity exists only for the lifetime of one in-flight request on the replica that inserted the map entry.

Phase 2b adds JetStream KV dedup (`mcp-reply-dedup`) for safe client retries — see `docs/identity/reply-correlation.md` §6. Dedup is orthogonal to this decision; the correlation map remains in-memory only.

## Consequences

### Positive

- **Simple mental model:** One replica owns one request end-to-end; the correlation map is the single source of truth for reply routing during the request lifetime.
- **Fast hot path:** Map insert and lookup are in-process; no NATS round-trip or JetStream read is required to correlate a backend reply. Backend forward uses core NATS publish, not inline `request_with_headers` tied to the ingress worker's client library state.
- **Observable state:** Per-replica gauge on map size (`gateway.reply.correlation.inflight`), span attributes on gateway inbox NUID, and audit events for orphaned replies give operators visibility subject-transform cannot expose.
- **Policy alignment:** Gateway evaluates policy **before** minting the backend inbox and inserting the map. Denied requests never allocate correlation entries or backend reply subjects.
- **Wire-format consistency:** Matches Wire-Format Pin 2 inbox naming and Pin 3 subscription topology documented in `docs/identity/reference-reply-inboxes.md` and `docs/identity/reference-queue-groups.md`.

### Negative

- **Replica failure loses in-flight requests:** If a gateway process dies after map insert but before client reply publish, the backend reply arrives at `_INBOX.gateway.{instance_id}.{nuid_g}` with no subscriber; the client observes a NATS timeout. No cross-replica recovery of orphaned work — clients must retry (with dedup in Phase 2b for mutating methods).
- **RAM-bounded correlation map:** Map size is capped by inflight semaphores (per `server_id` default 256, per tenant default 4096). Under saturation, new requests receive `-32105` `rate_limited` before map insert. Memory use scales with concurrent in-flight requests per replica, not with total cluster throughput.
- **No exactly-once delivery:** Core NATS does not guarantee client inbox delivery after gateway publish (failure mode b). Gateway audit may record success while the client times out.
- **Rolling deploy risk:** In-flight work on a draining replica is lost unless graceful shutdown waits up to `operation_timeout()` — proposed Phase 3 behaviour in the spec doc.

### Neutral

- **Horizontal scaling unchanged:** Queue group on `{prefix}.gateway.request.>` continues to distribute **new** ingress across replicas. Correlation does not require sticky sessions or partitioned consumers.
- **Per-instance inbox subscription is not queue-grouped:** Each replica direct-subscribes to `_INBOX.gateway.{my_instance_id}.>` per Wire-Format Pin 3. This is intentional — queue-grouping backend reply inboxes would break affinity and deliver replies to the wrong worker.
- **Client retry remains required:** Modes (a) and (b) cannot be hidden without exactly-once infrastructure. The gateway makes retries **safer** via KV dedup (Phase 2b), not invisible.
- **Session state stays separate:** Initialize context, ZedToken cache, and cancel/progress routing use `mcp-sessions` KV per `docs/identity/mcp-session-model.md`. Full reply maps are **not** replicated to KV.

## Alternatives considered

### (a) NATS subject-transform with inbox preservation

**Shape:** NATS server subject mapping rewrites gateway ingress to backend subjects while forwarding the client's original `reply-to` to the backend. Backend replies directly to `_INBOX.client.*`; gateway never holds a correlation map.

| Assessment | Detail |
|------------|--------|
| Pros | No in-memory map; theoretically survives gateway restart if backend reply is in flight to client inbox |
| Cons | Requires per-cluster NATS server configuration; transform runs outside gateway policy evaluation; bypasses Wire-Format Pin 2 gateway inbox; per-message audit and header injection (`mcp-instance-id`, `mcp-deadline-unix-ms`) harder to enforce; operator burden for every new environment |

**Rejected for Phase 1:** Gateway policy termination, pinned inbox shapes, and ops simplicity outweigh the elimination of process-local state. May be revisited only for non-MCP NATS proxy use cases outside this gateway.

### (b) JetStream-backed correlation store

**Shape:** Persist `{nuid_g} → ClientReplyTo` in JetStream KV (or a dedicated stream) so any replica can complete a reply if another replica dies mid-request.

| Assessment | Detail |
|------------|--------|
| Pros | Theoretically enables cross-replica reply completion after owner death |
| Cons | KV read/write latency on every ingress and backend reply (1–3 ms in-cluster minimum); contradicts explicit plan default in `docs/identity/mcp-session-model.md` ("Do not store in-flight inbox maps in KV"); race complexity without strong per-key locking; broadcast or watch fan-out if multiple replicas subscribe to completion |

**Rejected:** Latency and operational complexity on the hot path. JetStream KV is reserved for **dedup** records (Phase 2b), not full correlation state.

### (c) Fleet-wide reply broadcast subject

**Shape:** Replicas publish terminal responses to `mcp.gateway.reply.{request_id}`; whichever replica holds the client mapping delivers to the client.

**Rejected:** Requires replicated correlation map (same as b), new subject namespace, ACL surface, and fan-out cost scaling with replica count. Not present in subject grammar. See `docs/identity/reply-correlation.md` §3.2.

### (d) Sticky routing via JetStream consumer partition

**Shape:** Partition ingress by `session_id` or JSON-RPC `id` so retries land on the same replica that may still hold in-memory state.

**Rejected:** Conflicts with "No sticky routing for reply correlation" in `docs/identity/mcp-gateway-operator-overview.md` §4. Session affinity belongs in session KV (Phase 3 optional), not per-request JetStream partitions.

### (e) Phase 1 inline `request_with_headers` (status quo)

**Shape:** Preserve client `Message.reply` through the NATS client's built-in request/reply for the backend hop.

| Assessment | Detail |
|------------|--------|
| Pros | Shipped; minimal code |
| Cons | Implicit correlation in NATS client library; no explicit `_INBOX.gateway.{instance_id}.{nuid}`; harder to inject egress headers, enforce deadlines per map entry, or audit orphaned backend replies |

**Superseded by this ADR for Phase 2:** Phase 1 remains valid until explicit inbox subscription lands; wire-format pins already describe the target shape.

## Implementation notes

Design detail and algorithms are normative in `docs/identity/reply-correlation.md` and `docs/identity/reference-reply-inboxes.md`. This section records ADR-level pins for implementers.

### Correlation map

```rust
// Conceptual — newtypes at module boundary
HashMap<GatewayInboxNuid, ClientReplyTo>
```

| Component | Contract |
|-----------|----------|
| Key | `{nuid}` suffix of `_INBOX.gateway.{instance_id}.{nuid}` |
| Value | `ClientReplyTo { original_reply_to, request_ctx, span_ctx, deadline }` |
| Insert timing | Immediately before backend publish with gateway minted `reply-to` |
| Remove timing | Terminal client reply, terminal error publish, or deadline eviction |

### Bounded map size

Map entry count is bounded by the same inflight semaphores that gate backend fan-out (see [reference-rate-defaults.md](../identity/reference-rate-defaults.md)):

| Scope | Default cap | On exceed |
|-------|-------------|-----------|
| Per `server_id` | 256 | `-32105` `rate_limited`, scope `server` |
| Per tenant | 4096 | `-32105` `rate_limited`, scope `tenant` |

The correlation map cannot grow without a corresponding inflight permit.

### Eviction on deadline

Header `mcp-deadline-unix-ms` propagates client absolute deadlines; gateway clamps to `min(client_deadline, gateway_max_deadline)` before egress and stores the clamped value in `ClientReplyTo.deadline`.

When wall clock exceeds `deadline`:

1. Remove map entry for `{nuid_g}`.
2. Publish JSON-RPC `-32102` `backend_timeout` to `original_reply_to`.
3. Emit `{prefix}.audit.error.request.{method_root}` with `decision_reason: backend_timeout`.

Late backend replies after eviction must not resurrect entries.

### Audit `reply_orphaned` on late reply

If the backend publishes to `_INBOX.gateway.{instance_id}.{nuid_g}` after the map entry was removed (deadline eviction, successful completion, or duplicate reply):

| Step | Behaviour |
|------|-----------|
| Lookup | `{nuid_g}` not found |
| Client | No additional message |
| Backend bytes | Discarded |
| Audit | `{prefix}.audit.error.request.{method_root}` with `decision_reason: reply_orphaned` |
| Trace | Span event `gateway.reply.orphaned` |

Orphan handling is observability-only; it must not block the hot path.

### Subscription topology (Wire-Format Pin 3)

| Subscription | Queue group |
|--------------|-------------|
| `{prefix}.gateway.request.>` | `mcp-gateway` |
| `{prefix}.client.>` | `mcp-gateway-callbacks` |
| `_INBOX.gateway.{my_instance_id}.>` | **None** — direct subscribe only |

Cross-instance subscription to another replica's gateway inbox namespace is forbidden.

### Code and config surfaces

| Surface | Change |
|---------|--------|
| `trogon-mcp-gateway` | Per-instance inbox subscription at boot; explicit correlation map; replace inline backend `request_with_headers` with publish + map-driven reply handler |
| Headers | `mcp-instance-id` on egress; `mcp-producer-replica-id` on client replies; `mcp-deadline-unix-ms` clamp and store |
| Metrics | `gateway.reply.correlation.inflight` gauge; dedup counters in Phase 2b |
| Phase 2b | `mcp-reply-dedup` JetStream KV bucket — dedup only, not correlation map |

Bidirectional flows (server→client callbacks) repeat the same termination pattern per `docs/identity/bidirectional-enforcement.md`.

## Status of supporting work

| Item | Status | Notes |
|------|--------|-------|
| Spec: `docs/identity/reply-correlation.md` | **Done** | Block B paper artifact (2026-05-28); failure modes, dedup, idempotency, observability |
| Spec: `docs/identity/reference-reply-inboxes.md` | **Done** | Inbox shapes, map contract, deadline, orphaned reply audit |
| Spec: `docs/identity/reference-queue-groups.md` | **Done** | Pin 3 queue group vs direct subscribe |
| ADR 0009 (this document) | **Done** | Formal decision record |
| Phase 1 gateway (`trogon-mcp-gateway`) | **Shipped** | Inline `Message.reply`; no explicit map |
| Phase 2a: per-instance inbox + in-memory map | **Pending** | Target first implementation slice |
| Phase 2b: `mcp-reply-dedup` KV + reply envelope headers | **Pending** | Depends on 2a |
| Phase 3: drain-on-SIGTERM, cancel inflight keys | **Pending** | Operational hardening |

**Sign-off criteria for Block B item 2:** This ADR accepted; `docs/identity/reply-correlation.md` cited as design context; Phase 2a implementation tracked separately in gateway crate work.
