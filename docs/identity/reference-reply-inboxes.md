# Reply inbox naming reference

**Diátaxis:** reference (strict, exhaustive).

**Status:** Phase 1 contract. Reply inbox shapes are pinned.

**Related:** [Reference subject grammar](reference-subject-grammar.md), [Reply correlation](reply-correlation.md), [Reference audit envelope](reference-audit-envelope.md), [Failure-mode matrix](failure-mode-matrix.md), [MCP gateway operator overview](mcp-gateway-operator-overview.md).

This page is the single lookup for NATS reply-inbox tokens used by the MCP gateway to terminate request/reply correlation between clients, gateway workers, and backend MCP servers. Wire examples use default prefix `mcp`; substitute `{prefix}` when `MCP_PREFIX` is overridden.

**Implementation note:** Phase 1 gateway code (`trogon-mcp-gateway`) preserves the client `Message.reply` through an inline `request_with_headers` call. The inbox shapes and subscription topology below are the **pinned contract** for Phase 1 wire format and the target explicit correlation map in Phase 2. See [reply-correlation.md](reply-correlation.md) for today vs target behaviour.

---

## 1. The two inbox shapes

Wire-Format Pin 2:

```
_INBOX.gateway.{instance_id}.{nuid}        # gateway → backend reply correlation
_INBOX.client.{nuid}                       # client → gateway reply correlation (client-chosen)
```

| Token | Direction | Owner | Gateway subscribes? |
|---|---|---|---|
| `_INBOX.client.{nuid}` | Client → gateway (ingress reply) | Client NATS connection | **No** — gateway publishes only |
| `_INBOX.gateway.{instance_id}.{nuid}` | Gateway → backend (egress reply) | Gateway process that minted `{nuid}` | **Yes** — direct subscribe on owning instance only |

NATS sets the ingress reply token on `Message.reply` when the client uses request/reply publish APIs. The gateway **terminates** correlation: it does not forward the client inbox to the backend; it mints a gateway-side inbox and holds the mapping in process memory.

---

## 2. `{instance_id}` semantics

| Property | Value | Source |
|---|---|---|

### 2.1 Lifecycle

1. Process starts; gateway generates `{instance_id}` once.
2. Gateway begins periodic heartbeat publish to `{prefix}.control.gateway.heartbeat.{instance_id}`.
3. All gateway-minted backend reply inboxes for that process use `_INBOX.gateway.{instance_id}.{nuid}`.
4. Process exits; heartbeat stops; no subscriber remains on that inbox namespace.

### 2.2 Collision on restart

If `{instance_id}` collides after restart (extremely unlikely for NUID), prior in-flight requests are **already lost** because the process died. No recovery or cross-instance handoff is attempted.

Operators correlate replica loss via heartbeat cessation ([failure-mode-matrix.md](failure-mode-matrix.md) row 11, [mcp-session-model.md](mcp-session-model.md)).

---

## 3. Subscription topology

Each gateway instance subscribes **only** to:

```text
_INBOX.gateway.{my_instance_id}.>
```

| Rule | Value |
|---|---|
| Subscribe mode | Direct (core NATS subscribe) |
| Queue group | **None** |
| Wildcard | `>` matches all `{nuid}` tokens under this `{instance_id}` |
| Cross-instance | **Forbidden** — instance A must never subscribe to `_INBOX.gateway.{instance_B}.>` |

Ingress and callback fan-in still use queue groups:

| Subscription | Queue group |
|---|---|
| `{prefix}.gateway.request.>` | `mcp-gateway` |
| `{prefix}.client.>` | `mcp-gateway-callbacks` |
| `_INBOX.gateway.{my_instance_id}.>` | *(none — direct subscribe)* |

ACL summary for gateway service account ([nats-callout-plugin.md](nats-callout-plugin.md)): publish and subscribe on `_INBOX.gateway.>` scoped to the gateway role; clients publish/subscribe on `_INBOX.client.>` only.

---

## 4. Client inbox shape

The client inbox token is the **client's choice**. Conventional shape:

```text
_INBOX.client.{nuid}
```

| Property | Contract |
|---|---|
| Minting | Client NATS library or bridge |
| Gateway subscribe | **Never** on `_INBOX.client.*` |
| Gateway action | Publish JSON-RPC result or error to the `Message.reply` token captured at ingress |
| Preservation | Original client `reply-to` stored in correlation map under gateway-side `{nuid}` key |

Bidirectional flows (server → client callback, client → gateway callback response) repeat the same termination pattern: gateway mints `_INBOX.gateway.{instance_id}.{nuid}` for the downstream leg and maps back to the upstream caller inbox.

---

## 5. Correlation map

The gateway holds in-flight reply routing state **in memory per instance**. It is not replicated to JetStream KV ([reply-correlation.md](reply-correlation.md) §1.3).

### 5.1 Map shape

```rust
// Conceptual — newtypes at module boundary; not raw String across API surfaces.
HashMap<GatewayInboxNuid, ClientReplyTo>
```

| Field | Type (conceptual) | Purpose |
|---|---|---|
| Map key | `GatewayInboxNuid` — `{nuid}` segment of `_INBOX.gateway.{instance_id}.{nuid}` | Lookup when backend reply arrives |
| Map value | `ClientReplyTo` | Original client reply subject + request context |

`ClientReplyTo` record:

| Member | Purpose |
|---|---|
| `original_reply_to` | Client inbox (`_INBOX.client.{nuid_c}` or other client-chosen subject) |
| `request_ctx` | JSON-RPC `id`, ingress subject, server_id, method |
| `span_ctx` | W3C trace linkage for reply-path spans |
| `deadline` | Absolute deadline from `mcp-deadline-unix-ms` after gateway clamp |

Insert occurs immediately before publishing to `{prefix}.server.{server_id}.{method}` with `reply-to: _INBOX.gateway.{instance_id}.{nuid}`.

Remove occurs on:

- Successful client reply publish (terminal success).
- Terminal error publish to client (`-32102`, `-32103`, policy deny, etc.).
- Deadline expiry (see §6).

### 5.2 Bounded size

Map entry count is bounded by the same inflight semaphores that gate backend fan-out:

| Scope | Default cap | On exceed |
|---|---|---|
| Per `server_id` inflight | 256 | `-32105` `rate_limited`, scope `server` |
| Per tenant inflight | 4096 | `-32105` `rate_limited`, scope `tenant` |

The correlation map cannot grow without a corresponding inflight permit. Operators monitor `gateway.reply.correlation.inflight` **(proposed)** per replica ([reply-correlation.md](reply-correlation.md) §9.1).

### 5.3 Eviction on deadline

When wall clock exceeds `deadline`:

1. Gateway removes the map entry for `{nuid}`.
2. Gateway publishes JSON-RPC `-32102` `backend_timeout` to `original_reply_to` (if still reachable).
3. Gateway emits `{prefix}.audit.error.request.{method_root}` with `decision_reason: backend_timeout` **(proposed unified header)** ([reference-audit-envelope.md](reference-audit-envelope.md) §2.3).

Late backend replies must not resurrect evicted entries.

### 5.4 Orphaned backend reply

If the backend publishes to `_INBOX.gateway.{instance_id}.{nuid}` **after** the map entry was removed (deadline eviction, successful early completion, or duplicate reply):

| Step | Behaviour |
|---|---|
| Lookup | `{nuid}` not found in map |
| Client | No additional message (client already received `-32102` or terminal success) |
| Backend bytes | Discarded — not forwarded |
| Audit | `{prefix}.audit.error.request.{method_root}` with `decision_reason: reply_orphaned` **(proposed)** |
| Trace | Span event `gateway.reply.orphaned` with hashed gateway inbox **(proposed)** |

This is observability-only; orphan handling must not block the hot path.

---

## 6. Deadline propagation

Header: `mcp-deadline-unix-ms`.

| Hop | Action |
|---|---|
| Client → gateway | Client may set absolute deadline (Unix ms, integer string) |
| Gateway clamp | Gateway reduces to `min(client_deadline, gateway_max_deadline)` before egress |
| Gateway → backend | Clamped value copied on outbound headers |
| Correlation | Same value stored in `ClientReplyTo.deadline` |
| Timer | Gateway arms deadline timer per map entry |

On expiry before backend reply:

| Surface | Value |
|---|---|
| JSON-RPC code | `-32102` |
| Symbol | `backend_timeout` |
| `data` shape | `{ trace_id, server_id, elapsed_ms }` |
| Audit | `backend_timeout` / `{prefix}.audit.error.request.{method_root}` |

Timeout is **closed** — no synthetic success, no policy bypass ([failure-mode-matrix.md](failure-mode-matrix.md) row 8, invariant 7).

Phase 1 **(today)** uses `Config::operation_timeout()` from `mcp-nats` when the header is absent; header-aware clamp is the pinned Phase 1 wire contract ([reply-correlation.md](reply-correlation.md) §1.2).

---

## 7. End-to-end correlation flow

```
1. client publishes {prefix}.gateway.request.github.tools.call
                                       reply-to: _INBOX.client.{nuid_c}

2. gateway receives, applies policy, then publishes
   {prefix}.server.github.tools.call
                    reply-to: _INBOX.gateway.{instance_id}.{nuid_g}
                    mcp-instance-id: {instance_id}
                    mcp-deadline-unix-ms: {clamped_deadline}

3. gateway holds map: {nuid_g} → ClientReplyTo {
       original_reply_to: _INBOX.client.{nuid_c},
       request_ctx, span_ctx, deadline
   }

4. backend server replies to _INBOX.gateway.{instance_id}.{nuid_g}

5. gateway receives reply, looks up {nuid_g} → ClientReplyTo, applies response
   policy (redaction, list filtering), then publishes to
   _INBOX.client.{nuid_c}.
```

Sequence applies symmetrically to callback legs.

---

## 8. Failure modes

What the **client** observes for each failure class. Gateway audit and operator signals included for tracing.

| Failure | Client symptom | JSON-RPC code | Gateway audit / ops |
|---|---|---|---|
| Gateway instance dies mid-request | NATS request timeout; no JSON-RPC body | *(transport timeout — no gateway body)* | Heartbeat on `{prefix}.control.gateway.heartbeat.{instance_id}` stops; backend reply to `_INBOX.gateway.{dead_instance_id}.*` dropped (no subscriber); map lost |
| Backend never replies within deadline | JSON-RPC error on client inbox | `-32102` `backend_timeout` | `{prefix}.audit.error.request.{method_root}`; `decision_reason: backend_timeout` **(proposed)** |
| Correlation entry evicted (deadline fired) | JSON-RPC error on client inbox (same as timeout) | `-32102` `backend_timeout` | If backend replies later: `decision_reason: reply_orphaned` **(proposed)**; client unaffected |
| NATS partition (gateway ↔ backend) | NATS request timeout or gateway error if publish fails after accept | `-32102` if timer fires; `-32103` `backend_unreachable` if egress publish fails **(today partial)** | Instance degraded; queue subscription may drop ([failure-mode-matrix.md](failure-mode-matrix.md) row 11); client retries another gateway member |
| NATS partition (gateway ↔ client) after backend success | NATS request timeout | *(transport timeout)* | Gateway may have recorded `allow` audit while client sees timeout ([reply-correlation.md](reply-correlation.md) §2.2) |
| Duplicate client retry (same JSON-RPC `id`) | Zero, one, or two responses depending on dedup **(Phase 2+)** | `-32105` `duplicate_in_flight` **(proposed)** when dedup KV claims conflict | See [reply-correlation.md](reply-correlation.md) §6 |

**Retry guidance:** Clients must treat timeout after a mutating `tools/call` as **unknown execution** unless dedup or idempotent backend semantics apply. Queue-group ingress delivers retries to **any** healthy replica; only the replica that owns the in-flight map can complete the original attempt.

---

## 9. Why not queue-group the gateway inbox

Queue-grouping `_INBOX.gateway.{instance_id}.>` would deliver a backend reply to a **random** gateway worker in the group. Correlation state (`HashMap<GatewayInboxNuid, ClientReplyTo>`) lives in **process memory on the instance that minted `{nuid}`** and accepted the ingress message. A peer replica receiving the backend reply would find no map entry, could not recover the client's `original_reply_to`, and could not safely guess which client inbox to publish to. Replicating the full in-flight map to KV would add latency and complexity explicitly rejected for the hot path. Therefore the gateway inbox subscription is **direct per `{instance_id}`** while ingress remains queue-grouped for throughput and failover on **new** messages only ([mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md) §4).

---

## 10. Headers on the reply path

| Header | Reply-path role |
|---|---|
| `mcp-instance-id` | Backend and audit can attribute which gateway instance owns the inbox namespace |
| `mcp-deadline-unix-ms` | Drives map eviction and `-32102` |
| `mcp-correlation-id` | Opaque client correlator in audit; does not route replies |
| `traceparent` / `tracestate` | Span continuity across client → gateway → backend → gateway → client |

Full header table: [reference-nats-headers.md](reference-nats-headers.md).

---

## 11. NATS ACL patterns

Illustrative allow lists ([nats-callout-plugin.md](nats-callout-plugin.md)); adjust for `{prefix}` and tenancy model.

| Role | Subscribe | Publish |
|---|---|---|
| Client / edge bridge | `{prefix}.gateway.callback.{my_client_id}.>`, `_INBOX.client.>` | `{prefix}.gateway.request.>`, `_INBOX.client.>` |
| Backend MCP server | `{prefix}.server.{my_server_id}.>` | `{prefix}.server.{my_server_id}.>`, `_INBOX.gateway.>` (reply only) |
| Gateway service | `{prefix}.gateway.request.>`, `{prefix}.client.>`, `_INBOX.gateway.{my_instance_id}.>` | `{prefix}.server.>`, `{prefix}.gateway.callback.>`, `_INBOX.client.>` (client reply), `_INBOX.gateway.{my_instance_id}.>` (backend reply inbox mint) |

Backends publish to gateway inboxes; they do not subscribe to client inboxes.

---

## 12. Observability

| Signal | Purpose |
|---|---|
| `{prefix}.control.gateway.heartbeat.{instance_id}` | Liveness and instance_id discovery |
| `{prefix}.audit.error.request.{method_root}` | `backend_timeout`, `reply_orphaned` **(proposed)** |
| `gateway.reply.correlation.inflight` gauge **(proposed)** | Map size per replica |
| `gateway.backend_timeout` counter **(proposed)** | `-32102` emissions ([otel-wiring.md](otel-wiring.md)) |

Join operator traces to client logs via JSON-RPC `id`, `mcp-correlation-id`, and W3C `trace_id` ([reference-audit-envelope.md](reference-audit-envelope.md) §2.1).

---

## 13. Cross-references

| Document | Relevance |
|---|---|
| [reference-subject-grammar.md](reference-subject-grammar.md) | Ingress/egress subjects; control-plane heartbeat subject |
| [reply-correlation.md](reply-correlation.md) | Queue-group failure modes, dedup strategy, Phase 1 vs target |
| [reference-audit-envelope.md](reference-audit-envelope.md) | Audit subject grammar and proposed `decision_reason` values |
| [failure-mode-matrix.md](failure-mode-matrix.md) | Rows 8, 11 — backend timeout and NATS partition |
| [mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md) §4 | Operator summary of per-instance inbox model |

---

## 14. Phase alignment

| Capability | Phase 1 contract (pinned) | Implementation status |
|---|---|---|
| Inbox string shapes | `_INBOX.gateway.{instance_id}.{nuid}`, `_INBOX.client.{nuid}` | Shapes pinned; explicit mint **(Phase 2 code)** |
| Per-instance direct subscribe | Required | **(Phase 2 code)** |
| Correlation map in memory | Required | Implicit via NATS client **(today)**; explicit map **(Phase 2)** |
| `mcp-instance-id` header | Required on egress | **(Phase 2 code)** |
| `mcp-deadline-unix-ms` clamp | Required | Header pin; clamp logic **(Phase 2 code)** |
| `reply_orphaned` audit | Required when map miss on backend reply | **(proposed emitter)** |

Wire-format pins are stable across phases; implementation may lag without changing the contract.
