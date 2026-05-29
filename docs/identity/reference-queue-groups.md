# Queue group strategy reference

**Diátaxis:** reference (strict, exhaustive).

**Status banner:** Phase 1 contract. Queue group names are pinned.

**Related:** [reference-subject-grammar.md](reference-subject-grammar.md), [reference-reply-inboxes.md](reference-reply-inboxes.md), [rate-limiting.md](rate-limiting.md), [reference-error-codes.md](reference-error-codes.md), [reply-correlation.md](reply-correlation.md), [mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md).

**Implementation anchors:**

| Surface | Env override | Default queue group | Subscribe pattern | Source |
|---|---|---|---|---|
| Gateway request ingress | `MCP_GATEWAY_QUEUE_GROUP` | `mcp-gateway` | `{prefix}.gateway.request.>` | `trogon-mcp-gateway/src/config.rs`, `gateway.rs` |

Unless noted **(today)**, behaviour described here is the Phase 1 wire contract. Phase 1 shipped code today implements only the request ingress row; callback lane, per-instance inbox, and plugin queue groups are pinned for Block H reference closure and Phase 2 wiring.

Default prefix examples use `mcp`; substitute `{prefix}` when `MCP_PREFIX` is overridden ([reference-subject-grammar.md §1](reference-subject-grammar.md#1-prefix)).

---

## 1. Pinned queue-group table (Wire-Format Pin 3)

Reproduced verbatim:

| Subscription | Queue group name | Reason |
|---|---|---|
| `mcp.gateway.request.>` | `mcp-gateway` | Single group; any healthy instance can serve any request. |
| `mcp.client.>` | `mcp-gateway-callbacks` | Same instances, separate group so request and callback fairness are independent. |
| `_INBOX.gateway.{my_instance_id}.>` | *(none — direct subscribe)* | Per-instance; queue-grouping would break correlation. |
| `mcp.plugin.{plugin_name}` | `mcp-plugin-{plugin_name}` | Plugin authors get queue-group scale automatically. |

**Prefix substitution:** replace the `mcp.` prefix with `{MCP_PREFIX}.` when the deployment overrides the default. Queue group **names** are not prefixed — they are cluster-global strings shared by all gateway replicas in the same NATS account.

**Environment override (request ingress only):** `MCP_GATEWAY_QUEUE_GROUP` or `--queue-group` on `trogon-mcp-gateway`. Default `mcp-gateway`. Operators who override must keep all replicas on the same group name or messages will not load-balance correctly.

---

## 2. Why separate request and callback groups

Gateway workers subscribe to two load-balanced surfaces:

1. **Request path** — clients publish to `{prefix}.gateway.request.>`; the gateway queue group `mcp-gateway` competes for those messages.
2. **Callback path** — backend MCP servers publish server-initiated MCP methods to `{prefix}.client.{client_id}.{method}`; the gateway listens on `{prefix}.client.>` under queue group `mcp-gateway-callbacks`.

Both groups typically run in the **same gateway process** today, but NATS treats them as independent subscriber pools. A burst of client-initiated `tools/call` traffic cannot starve server-initiated callbacks (`sampling/createMessage`, `elicitation/create`, progress notifications, list-changed notifications) because callback messages compete only within `mcp-gateway-callbacks`, not against the request backlog in `mcp-gateway`.

**Fairness independence:** NATS queue-group delivery is fair **within** a queue group name. Mixing requests and callbacks under one group would let a heavy request flood delay time-sensitive server→client work. Separate groups preserve independent NATS-level scheduling while still allowing horizontal scale of the same binary for both lanes.

**Operational note:** If callback lag rises while request latency is flat, inspect `mcp-gateway-callbacks` consumer count and callback-side inflight separately from request ingress ([mcp-gateway-operator-overview.md §7.1](mcp-gateway-operator-overview.md#71-horizontal-scaling)).

---

## 3. Why no queue group on `_INBOX.gateway.{my_instance_id}.>`

Reply correlation for gateway→backend request/reply is **instance-local**:

```
Client publish  →  gateway (any replica via mcp-gateway)
Gateway publish →  backend with reply-to: _INBOX.gateway.{instance_id}.{nuid_g}
Backend reply   →  _INBOX.gateway.{instance_id}.{nuid_g}
Gateway lookup  →  in-memory map: nuid_g → { client reply inbox, span, deadline, … }
Gateway publish →  client original reply inbox
```

The `{instance_id}` is a NUID generated at gateway boot. Each process subscribes **directly** (no queue group) to `_INBOX.gateway.{my_instance_id}.>` so that backend replies land on the replica that created the correlation entry.

**Why queue-grouping breaks this:** NATS queue groups deliver each message to **one** member of the group. If multiple gateway replicas shared a queue group on `_INBOX.gateway.*`, a backend reply could arrive at a replica that never saw the original ingress message and holds no correlation map entry. The reply would be dropped or mis-handled.

**Contrast with request ingress:** Request messages are stateless at pickup — any replica can authenticate, evaluate policy, and forward. Reply inboxes are stateful — only the originating replica owns the map entry until the request completes or times out.

See [reference-reply-inboxes.md](reference-reply-inboxes.md) and [reply-correlation.md](reply-correlation.md) for inbox naming, header `mcp-instance-id`, and dedup semantics.

---

## 4. Plugin queue groups

Tier-2.5 NATS-callout policy plugins use a **dedicated queue group per plugin name**:

| Published subject | Queue group | Scale model |
|---|---|---|
| `{prefix}.plugin.{plugin_name}` | `mcp-plugin-{plugin_name}` | Competing consumers; one message handled by one plugin worker |

**Naming rule:** queue group = literal prefix `mcp-plugin-` concatenated with `{plugin_name}` exactly as it appears in the subject's final token.

**`{plugin_name}` validation:** single NATS subject token matching `[a-z0-9-]{1,64}` — lowercase ASCII letters, digits, and hyphen only; length 1–64 inclusive. This is a Phase 1 pin on top of the general token rules in [reference-subject-grammar.md §3.1–§3.3](reference-subject-grammar.md#3-token-rules) (no dots, wildcards, or whitespace). Examples:

| `{plugin_name}` | Queue group | Valid |
|---|---|---|
| `pii-redactor` | `mcp-plugin-pii-redactor` | yes |
| `tenant-acme-hooks` | `mcp-plugin-tenant-acme-hooks` | yes |
| `PII_Redactor` | — | no (uppercase) |
| `my.plugin` | — | no (dot) |

**Plugin author checklist:**

1. Subscribe with `queue_subscribe("{prefix}.plugin.{plugin_name}", "mcp-plugin-{plugin_name}")`.
2. Publish replies to the gateway-provided reply inbox on the request message (standard NATS request/reply).
3. Run N replicas behind the same queue group name for horizontal scale; NATS delivers each plugin invocation to one healthy worker.
4. Align NATS account ACLs: publish on `{prefix}.plugin.>` is gateway-only; plugin principals subscribe on their `{prefix}.plugin.{plugin_name}` subtree.

**failClosed default:** if no plugin consumer replies within the configured deadline, the gateway treats the policy tier as deny ([nats-callout-plugin.md](nats-callout-plugin.md)).

---

## 5. Backpressure (inflight semaphores)

Queue groups distribute **work** across replicas; they do not cap **concurrency** per downstream target. Inflight limits are enforced inside the gateway by semaphores, independent of NATS queue depth.

Reproduced and [rate-limiting.md § Reference — platform default limits](rate-limiting.md#reference--platform-default-limits-from-plan-9):

| Scope | Default cap | Config surface | On saturation |
|---|---|---|---|
| Per `server_id` inflight | **256** | bundle / `mcp-gateway-config` KV | JSON-RPC `-32105` `rate_limited`, `data.scope = "server"`, `data.retry_after_ms` |
| Per tenant inflight | **4096** | bundle / `mcp-gateway-config` KV | JSON-RPC `-32105` `rate_limited`, `data.scope = "tenant"`, `data.retry_after_ms` |

**Saturation response shape** ([reference-error-codes.md](reference-error-codes.md), [rate-limiting.md § Client JSON-RPC error shape](rate-limiting.md#client-json-rpc-error-shape)):

```json
{
  "jsonrpc": "2.0",
  "id": "<client id>",
  "error": {
    "code": -32105,
    "message": "rate_limited",
    "data": {
      "scope": "server",
      "trace_id": "…",
      "retry_after_ms": 4200
    }
  }
}
```

**Semantics:**

- Inflight semaphores count **in-process** outstanding forwards to a given `server_id` or tenant, not NATS queue depth.
- `retry_after_ms` is a client hint derived from oldest inflight age or token-bucket time-to-next-token depending on limit class ([rate-limiting.md](rate-limiting.md)).
- Caps are **configurable per server** in the policy bundle and/or `mcp-gateway-config` KV keys (`inflight.server`, `inflight.tenant`).
- Inflight caps are **orthogonal** to token-bucket rate budgets (`rate.acquire`, per-`jwt.sub` windows). A request may pass rate acquire yet still receive `-32105` on inflight saturation.

**Fail-closed posture:** saturation is not an authz bypass. See [failure-mode-matrix.md](failure-mode-matrix.md) row 10 (gateway worker saturated).

---

## 6. Why per-target semaphore over per-method queue-group sprawl

An alternative design would create a separate NATS queue group per MCP method or per `server_id` (for example `mcp-gateway-github-tools-call`). That would push backpressure to NATS delivery order and multiply operational surfaces without adding correctness.

Phase 1 instead uses **one request queue group** (`mcp-gateway`) plus **in-process per-`server_id` semaphores**. This bounds head-of-line blocking — a slow `tools/call` storm against one backend cannot occupy all 256 slots for that server while other servers remain available — without requiring operators to register and ACL dozens of queue group names. Callback fairness remains a **second** pinned group (`mcp-gateway-callbacks`), not N method-specific groups. Plugin scale remains **per-plugin** (`mcp-plugin-{plugin_name}`) because plugins are independently deployed services with distinct SLOs, not methods on a shared gateway binary.

---

## 7. Operator guide — scaling gateway replicas

### 7.1 What queue groups scale

| Concern | Scaled by | Not scaled by |
|---|---|---|
| Client request throughput | More `trogon-mcp-gateway` replicas in `mcp-gateway` on `{prefix}.gateway.request.>` | Adding backend MCP servers |
| Server→client callback throughput | More replicas in `mcp-gateway-callbacks` on `{prefix}.client.>` | Request replica count alone (same process today, separate NATS group) |
| Reply correlation | **Not** queue-group scaled — each replica adds its own `_INBOX.gateway.{instance_id}.>` direct subscription | Increasing `mcp-gateway` members does not shard inboxes |
| Per-target backpressure | Raising inflight caps in bundle/KV | Adding replicas without raising caps (each replica enforces local semaphores; tenant cap is cluster-wide via KV) |

### 7.2 Horizontal scaling characteristics

**Adding gateway replicas:**

- Increases aggregate CPU and parallel policy evaluation for ingress messages NATS assigns to the `mcp-gateway` group.
- Adds one new `_INBOX.gateway.{instance_id}.>` namespace per replica (direct subscribe, no queue group).
- Does **not** require sticky sessions for the default JetStream KV session model ([mcp-session-model.md](mcp-session-model.md)).
- Does **not** multiply inflight allowance per `server_id` unless config caps are raised — default 256 is **per gateway process** toward that server.

**When to add replicas** ([mcp-gateway-operator-overview.md §7.1](mcp-gateway-operator-overview.md#71-horizontal-scaling)):

- Gateway CPU or inflight saturation while backend latency is flat.
- Growing P99 gateway span duration with flat SpiceDB and backend times.
- Observable queue-group consumer lag (see §9 Diagnostics).

**When replicas do not help:**

- Single saturated backend MCP server.
- SpiceDB or STS rate limits.
- JetStream audit stream disk bandwidth.
- Hitting per-tenant inflight cap (4096) cluster-wide — requires cap tuning or tenant traffic shaping, not only more gateway pods.

### 7.3 NATS message-distribution policy

Queue-group delivery is implemented by the NATS server: when multiple subscribers share the same queue group name on a subject pattern, each published message is delivered to **exactly one** subscriber in the group ([NATS queue subscribers](https://docs.nats.io/nats-concepts/core-nats/queue#queue-groups)).

**Implications for operators:**

- All gateway replicas **must** use identical queue group strings (`mcp-gateway`, `mcp-gateway-callbacks`) within an account.
- Replicas in **different** queue group names on the same subject do not load-balance together — they each receive all messages (duplicate processing).
- Distribution fairness is **best-effort** at the NATS server; extremely hot consumers may receive slightly more messages depending on server version and connection count. Monitor per-replica lag and CPU rather than assuming perfect uniformity.
- Queue groups provide **competing consumer** semantics, not **partitioned consumer** semantics (contrast with JetStream ordered consumers).

**Relationship to control-plane heartbeats:** each replica publishes `mcp.control.gateway.heartbeat.{instance_id}` so operators can enumerate live instance IDs for inbox ACL templates and correlation debugging.

---

## 8. Diagnostics — verifying queue group registration

Commands below are **illustrative**. Exact CLI flags vary by NATS server version and whether the deployment uses the `nats` CLI, monitoring HTTP port, or `$SYS` requests.

### 8.1 Confirm gateway replicas share the expected group

**Goal:** every healthy `trogon-mcp-gateway` pod appears as a queue subscriber on `{prefix}.gateway.request.>` with queue group `mcp-gateway`.

Illustrative checks:

```bash
# NATS CLI (illustrative) — list connections filtered by client name
nats server report connections --filter="trogon-mcp-gateway"

# Monitoring endpoint (illustrative) — subscription interest for queue group
curl -s "http://${NATS_MONITOR_HOST}:8222/subsz?subject=mcp.gateway.request.%3E&queue=mcp-gateway"
```

Expected signals:

- Connection count matches replica count (minus rolling-update drift).
- Queue group field equals `mcp-gateway` (or your `MCP_GATEWAY_QUEUE_GROUP` override applied uniformly).
- Subject interest includes `{prefix}.gateway.request.>`.

### 8.2 Callback lane

Repeat the same pattern for `{prefix}.client.>` with queue group `mcp-gateway-callbacks`. Absence of any callback subscriber while backends emit client-lane traffic produces `-32103 backend_unreachable`-class failures for server-initiated MCP methods.

### 8.3 Per-instance inbox (must NOT show a queue group)

Each gateway connection should show a **plain** (non-queue) subscription to `_INBOX.gateway.{instance_id}.>` matching the process `mcp-instance-id` / heartbeat subject. If this subscription appears under a queue group name, correlation is misconfigured.

### 8.4 Plugin workers

For plugin `{plugin_name}`:

```bash
# Illustrative — verify plugin queue group members
nats server report connections --filter="mcp-plugin-${PLUGIN_NAME}"
curl -s "http://${NATS_MONITOR_HOST}:8222/subsz?subject=mcp.plugin.${PLUGIN_NAME}&queue=mcp-plugin-${PLUGIN_NAME}"
```

Zero subscribers with gateway plugin tier enabled yields failClosed deny on plugin invocations.

### 8.5 JetStream vs core NATS

Gateway ingress uses **core NATS** queue subscribers, not JetStream durable consumers. Do not use `nats consumer info` for `mcp-gateway` — that command applies to JetStream pull/push consumers on streams. Use connection/subscription introspection (`subsz`, `$SYS.REQ.SERVER.PING.SUBSZ`, or operator dashboards) instead.

### 8.6 Load smoke

End-to-end verification:

1. Publish a gated MCP request with reply inbox from a staging client.
2. Confirm exactly one gateway replica handles each message (audit `instance_id` field or trace span).
3. Scale replicas up; confirm throughput rises until another bottleneck dominates.
4. Induce inflight saturation against one `server_id`; confirm `-32105` with `scope=server` without affecting unrelated servers ([rate-limiting.md](rate-limiting.md)).

---

## 9. Configuration reference

| Setting | Default | Override | Applies to |
|---|---|---|---|
| Request queue group | `mcp-gateway` | `MCP_GATEWAY_QUEUE_GROUP`, `--queue-group` | `{prefix}.gateway.request.>` |
| Callback queue group | `mcp-gateway-callbacks` | *(pinned Phase 1 — no env override)* | `{prefix}.client.>` |
| Plugin queue group | `mcp-plugin-{plugin_name}` | *(derived — not overridable)* | `{prefix}.plugin.{plugin_name}` |
| Per-`server_id` inflight | 256 | bundle / KV `inflight.server` | Gateway semaphore |
| Per-tenant inflight | 4096 | bundle / KV `inflight.tenant` | Gateway semaphore + KV |

---

## 10. Cross-references

| Document | Relevance |
|---|---|
| [reference-reply-inboxes.md](reference-reply-inboxes.md) | `_INBOX.gateway.{instance_id}.{nuid}` naming; why direct subscribe |
| [rate-limiting.md](rate-limiting.md) | Inflight vs token-bucket layers; `-32105` JSON-RPC shape; hybrid KV sync |
| [reference-error-codes.md](reference-error-codes.md) | `-32105 rate_limited`, `-32103 backend_unreachable` (no queue consumers) |
| [reference-subject-grammar.md](reference-subject-grammar.md) | Subject patterns; token rules; §5 queue group summary |
| [reply-correlation.md](reply-correlation.md) | In-memory correlation map; HA queue-group worker explanation |
| [mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md) | Scaling heuristics; day-2 checklist queue group line |
| [failure-mode-matrix.md](failure-mode-matrix.md) | Row 10 saturation semantics |
| [nats-callout-plugin.md](nats-callout-plugin.md) | Tier-2.5 plugin subject and entitlement model |

---

## Changelog

| Date | Change |
|---|---|
| 2026-05-28 | Initial Phase 1 reference (Wire-Format Pin 3 closure, Block H) |
