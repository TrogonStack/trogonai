# A2A streaming back-pressure — operator & implementer guide

How task event streaming stays non-blocking for agents, what JetStream policy to run on **`A2A_EVENTS`**, and how gateway-side pull consumers relate to agent-side **`Bridge`** limits.

## Related documents

| Document | Purpose |
|----------|---------|
| [A2A plan](../A2A_PLAN.md) | Streaming semantics, working surface (what ships today) |
| [A2A TODO](../A2A_TODO.md) | Phase 2 — gateway unary deadline and policy depth remain open |
| [A2A pending decisions](../A2A_PENDING_DECISION.md) | Landed §5 — pull consumer flow control + `discard=old` |
| [Per-Account JetStream assets](./A2A_JETSTREAM_ACCOUNT_STREAMS.md) | **`A2A_EVENTS`** provisioning reference |
| [Gateway roadmap](./A2A_GATEWAY_ROADMAP.md) | Gateway streaming egress (env-gated pull consumer) |
| [Runtime env](./A2A_RUNTIME_ENV.md) | **`A2A_MAX_CONCURRENT_CLIENT_TASKS`** for agent **`Bridge`** |
| [Documentation index](./A2A_DOCS_INDEX.md) | Hub linking operator and design docs |

---

## Problem

Agents publish **`TaskStatusUpdateEvent`** / **`TaskArtifactUpdateEvent`** JSON to per-task JetStream subjects (`{prefix}.task.{task_id}.events.{req_id}`). Those messages land in the shared Account stream **`A2A_EVENTS`**.

Downstream paths read the same stream:

- **Callers** — `message/stream` bootstrap + JetStream consumer, or `tasks/resubscribe` replay from `last_seq + 1`.
- **Future gateway** — rewrite/redact/audit pipe from stream to caller inbox (see [A2A plan](../A2A_PLAN.md) §Streaming).
- **Push targets** — terminal delivery reads from the agent pump, not the stream consumer directly.

If a slow or crashed consumer caused JetStream to block publishers, agent handlers would stall while emitting task updates — violating the A2A contract that agents keep working independently of client read speed.

---

## Stream policy: `retention=interest`, `discard=old`

**Target policy** (landed in [A2A pending decisions](../A2A_PENDING_DECISION.md) §5; tracked in [A2A TODO](../A2A_TODO.md) Phase 2):

| Setting | Value | Plain-language effect |
|---------|-------|------------------------|
| **`retention`** | **`interest`** | Messages are kept only while at least one active consumer has interest in them. When nobody is consuming, JetStream can drop data instead of holding it forever. |
| **`discard`** | **`old`** | When the stream hits a size or age limit, **oldest** messages are removed first — not the newest. A slow reader loses history; the agent's **next** publish still succeeds. |
| **`max_age`** | **`24h`** (baseline) | Replay/resubscribe window; operators may extend per Account for audit needs. |

**Why this pair:** With **`limits`** retention, the stream keeps messages until **`max_age`** / byte limits even when no consumer cares — agents can still publish, but storage grows with every task. With **`interest`**, unconsumed backlog can be reclaimed once consumer interest ends (for example after a client disconnect and ephemeral consumer expiry). **`discard=old`** ensures that when pressure hits, publishers are never blocked waiting for a slow consumer to drain — the stream sheds the oldest events inside the retention window instead.

**In-tree today:** `provision_streams` / `A2aStream::Events` sets **`retention=interest`**, **`discard=old`**, and **`max_age=24h`** (override **`A2A_EVENTS_MAX_AGE_SECS`** per Account). Gateway pull egress ships env-gated in `a2a-gateway` (`A2A_GATEWAY_EVENTS_PULL=on`). See [Runtime env](./A2A_RUNTIME_ENV.md) §`a2a-gateway`.

---

## Two layers of back-pressure

Back-pressure is split between **agent ingress** (how many RPCs the **`Bridge`** accepts) and **stream egress** (how fast consumers pull and ack). They solve different problems.

```text
Caller ──► Bridge (semaphore) ──► handler ──► JetStream publish ──► A2A_EVENTS
                                                                    │
                     future gateway pull consumer ◄──────────────────┘
                     (flow control, max_ack_pending)
                                    │
                                    ▼
                              caller inbox / SSE
```

### Agent `Bridge`: `max_concurrent_client_tasks`

Shipped today in **`a2a-nats`** agent **`Bridge`** ([A2A plan](../A2A_PLAN.md) §Working surface):

- **`Config::max_concurrent_client_tasks`** (default **256**, env **`A2A_MAX_CONCURRENT_CLIENT_TASKS`**) backs a tokio **`Semaphore`** on the dispatch loop.
- Each inbound agent RPC (`message/send`, `message/stream`, `tasks/*`, …) **acquires one permit** before **`dispatch`** runs and **releases it when `dispatch` returns**.
- For **`message/stream`**, `dispatch` returns after the bootstrap JSON-RPC reply and the background event pump is spawned — the permit is **not** held for the full task lifetime. Long-running streams are tracked separately via **`InFlightTasks`** (cancellation on **`tasks/cancel`**).

**Operator intent:** cap how many concurrent agent RPC handlers run at once so a connection storm cannot exhaust agent memory or task threads. This is **ingress** back-pressure on the agent, not JetStream consumer flow control.

**Relationship to gateway limits:** When the gateway ships per-caller streaming quotas (plan §Policy — max concurrent streaming tasks per caller per agent), that policy sits at **ingress** alongside auth. The **`Bridge`** semaphore remains the **last line of defense** on the agent process regardless of gateway enforcement — tune it for agent capacity, not caller fairness.

### Gateway egress: pull consumer with flow control (shipped, env-gated)

**Shipped in `a2a-gateway`** when **`A2A_GATEWAY_EVENTS_PULL=on`** (default off). A durable pull consumer on **`A2A_EVENTS`** (`{prefix}.task.*.events.*`) fetches with batch + heartbeat flow control, forwards to **`{prefix}.gateway.egress.{req_id}`**, and acks explicitly. Full rewrite/redact and caller inbox wiring remain future work alongside ingress policy.

**Target shape** for per-RPC ephemeral consumers when gateway owns the full caller pipe ([A2A plan](../A2A_PLAN.md) §Streaming):

1. **`message/stream`** — gateway forwards to agent, then creates an **ephemeral pull consumer** on `{prefix}.task.{task_id}.events.*` (or req-scoped filter), delivers to the caller reply inbox after rewrite/redact.
2. **`tasks/resubscribe`** — new ephemeral consumer starting at **`last_seq + 1`** (same semantics as today’s client-side `resubscribe_consumer`).
3. **Flow control** — enable JetStream **consumer flow control** so the server stops pushing batches when the gateway’s fetch loop falls behind; slow caller-side consumption throttles **fetch**, not agent **publish**.
4. **`max_ack_pending`** — set to a bounded value (order of tens, not thousands) so unacked messages in flight match gateway rewrite/forward capacity. Explicit ack after successful forward to caller inbox (same ack discipline as today’s `Client` event pump).

**Sketch — gateway pull consumer (illustrative, not in-tree):**

```text
# Ephemeral consumer on A2A_EVENTS for one task stream
filter_subject:  {prefix}.task.{task_id}.events.*
deliver_policy:  all | by_start_sequence (resubscribe)
ack_policy:      explicit
replay_policy:   instant
flow_control:    true
max_ack_pending: 32          # tune to gateway worker pool / rewrite latency
inactive_threshold: 5m       # matches in-tree INACTIVE_THRESHOLD (consumer hygiene)
```

**Consumer hygiene (shipped on client path):** ephemeral consumers set **`inactive_threshold = 5m`** so crashed clients do not leak server-side consumer metadata ([A2A plan](../A2A_PLAN.md) §Working surface). Gateway consumers should reuse the same threshold.

---

## Shipped today vs planned

| Area | Shipped today | Planned |
|------|---------------|---------|
| **`message/stream` / `tasks/resubscribe`** | End-to-end via agent **`Bridge`** + **`a2a_nats::Client`**; bootstrap reply + JetStream pump | Gateway-owned consumer-to-caller pipe with rewrite/audit |
| **Pull consumers (client path)** | Ephemeral pull configs (`stream_events_consumer`, `resubscribe_consumer`); explicit ack; **`inactive_threshold = 5m`** | Same semantics at gateway egress + per-RPC ephemeral consumers |
| **Gateway pull egress** | Durable consumer on **`A2A_EVENTS`** when **`A2A_GATEWAY_EVENTS_PULL=on`**; fetch batch + heartbeat; forward to **`{prefix}.gateway.egress.{req_id}`** | Rewrite/redact, caller inbox delivery, ingress-coordinated ephemeral consumers |
| **`A2A_EVENTS` discard** | **`discard=old`** in provisioner | unchanged |
| **`A2A_EVENTS` retention** | **`interest`** in provisioner | unchanged |
| **Agent ingress limit** | **`Bridge`** semaphore / **`A2A_MAX_CONCURRENT_CLIENT_TASKS`** | Gateway per-caller streaming quotas (policy bundle) in addition |
| **Gateway streaming (full pipe)** | Env-gated durable pull + egress subject forward only | Pull consumer per stream/resubscribe with rewrite/audit at ingress |
| **`message/send` deadline** | Agent-side operation timeout; gateway unary deadline via **`A2A_GATEWAY_UNARY_DEADLINE_SECS`** | Broader gateway lifecycle deadlines ([A2A TODO](../A2A_TODO.md) Phase 2) |

Enable gateway pull egress with **`A2A_GATEWAY_EVENTS_PULL=on`** after verifying **`A2A_EVENTS`** stream policy on the tenant Account.

---

## Operator verification

Connect with JetStream read access inside the tenant Account:

```bash
# Stream policy
nats --context TENANT stream info A2A_EVENTS

# Expect: discard old; retention interest
# Subject filter: a2a.task.*.events.*  (adjust prefix if non-default)
```

**Agent process:**

```bash
# Default concurrent RPC cap = 256; lower on small agents
export A2A_MAX_CONCURRENT_CLIENT_TASKS=64
```

**Symptoms when misconfigured:**

| Symptom | Likely cause |
|---------|----------------|
| Agent handler latency spikes under load | **`A2A_MAX_CONCURRENT_CLIENT_TASKS`** too high for agent CPU/memory, or handler blocking on I/O |
| Clients miss events after slow read | Expected with **`discard=old`** — resubscribe with last known **`last_seq`** within **`max_age`** |
| Stream storage grows with no active consumers | **`retention=limits`** still retaining — migrate to **`interest`** per landed decision |
| Orphan JetStream consumers after client crash | Should auto-expire after **5m** inactive; verify **`inactive_threshold`** on consumers |

---

## Implementer notes

- **Publish path:** agent **`message_stream`** pump publishes via JetStream (`TaskEventsSubject`); failures surface in agent logs, not caller blocking.
- **Client path:** `build_event_stream` acks after each message before yielding to the application stream — slow app readers back up the unbounded mpsc channel today; gateway flow control addresses this at the JetStream boundary for the ingress pipe.
- **Audit:** `TaskLifecycleEnvelope` emission on streamed state transitions is agent-side today; gateway **`stream_consumer`** on forward decision-site **`AuditEnvelope`** is populated for SSE-shaped methods (`message.stream`, `tasks.resubscribe`) as `gateway.{agent_id}.{method_dots}` for downstream correlation ([A2A plan](../A2A_PLAN.md) §Audit).

When implementing gateway streaming, treat [Gateway roadmap](./A2A_GATEWAY_ROADMAP.md) coordination items (JWT, Tier 1, ingress audit) as prerequisites for production egress — flow control alone does not replace auth or policy.
