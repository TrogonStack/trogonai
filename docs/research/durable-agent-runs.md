# Durable Agent Runs

## Context

`trogon-agent` receives trigger events from NATS JetStream (GitHub webhooks, Linear events,
Datadog alerts, cron ticks) and runs an agentic loop: multiple sequential LLM calls to Anthropic
with tool execution between them — posting GitHub comments, creating Linear issues, sending Slack
messages, etc. A single run can take several minutes.

The core durability gap: if the process crashes mid-run, JetStream redelivers the original
trigger and the agent restarts from scratch — re-executing every LLM call and every tool call
already completed. There is no checkpoint, no idempotency mechanism, and no recovery path.

---

## Alternatives Evaluated

### Temporal

Temporal workflows are event-sourced: every step is journaled before execution. On crash, the
workflow worker replays the journal without re-executing completed activities. The strongest
durability guarantee of any option evaluated.

**Discarded:**
- The Rust SDK (`temporalio-sdk`) is explicitly pre-release with unstable API as of April 2026.
- Self-hosting requires PostgreSQL or Cassandra, Elasticsearch, and four Temporal service
  components. Disproportionate infrastructure for this use case.
- Workflow determinism constraints add significant operational burden: all non-deterministic
  operations must be wrapped in Activities, and changing activity order in a deployed workflow
  breaks in-flight executions.

### Restate (restate.dev)

Real Rust SDK (`restate-sdk` v0.9.0, February 2026, actively maintained). Sits as a reverse
proxy and maintains a per-invocation journal backed by RocksDB. Side effects wrapped in
`ctx.run()` are journaled atomically — on retry, the cached result is replayed.

**Deferred, not discarded:**
- Requires the agent to become an HTTP service, inverting the current NATS pull-consumer model.
  A NATS→Restate bridge would be needed.
- SDK is pre-1.0 with API instability warnings.
- Best medium-term option once SDK reaches 1.0. The `ctx.run()` model maps directly to wrapping
  each LLM call and tool call.

### Resonate HQ (resonatehq.io)

Implements the Durable Promises specification: a promise is a unit of async work with a unique
ID stored durably. If the process crashes, the promise survives and can be resumed.

**Discarded:**
- No Rust SDK exists as of April 2026. The co-founder announced SDK design work in March 2026
  but nothing has shipped.
- Requires a separate server process. Not usable today in Rust.

### iopsystems/durable

Embedded Rust library for durable execution backed by PostgreSQL. No separate server.

**Discarded:**
- Requires PostgreSQL. The stack is NATS-first; adding a relational database introduces
  infrastructure the project does not have.
- 48 stars — meaningful abandonment risk.

### Pure JetStream Consumer Tuning

Tuning `ack_wait`, `max_deliver`, and using `Nats-Msg-Id` publisher-side deduplication.

**Discarded as a complete solution:**
- Manages *delivery* of the trigger message, not *progress* within a single delivery. A crash
  after 8 LLM turns causes redelivery of the original message and restart from turn 1.
- `Nats-Msg-Id` deduplication is publisher-side only: it prevents the same event being ingested
  twice, not re-execution of work during consumer-side retry.
- Necessary foundation, but not sufficient alone.

### Event Sourcing / Saga Over NATS

Decompose the agent loop into discrete NATS messages: one per LLM turn, one per tool call. Each
step is independently ACK'd. State lives in NATS KV between steps.

**Discarded as primary approach:**
- Largest code change of any pure-NATS option: requires a coordinator, transforms the agent loop
  into a distributed state machine.
- Delivers equivalent practical durability to the chosen solution at 3–5× the implementation
  cost.
- Tool idempotency is required regardless of approach.
- Better suited as a future architectural evolution.

---

## Chosen Solution: Durable Promises on NATS KV

The Durable Promises concept — a promise as a durable unit of work with a unique ID — implemented
directly on NATS KV. No external infrastructure. NATS KV is already used throughout the codebase
(`SessionStore`, `RunStore`, `AutomationStore`).

### Core Principle

Before executing any step, check if a result already exists in KV. If it does, return the cached
result without re-executing. If it does not, execute, store the result, then continue. On restart,
the run resumes from the last checkpoint rather than from scratch.

```
For each step in the AgentLoop:
  1. kv.get(step_key)        → if result exists: return it (no re-execution)
  2. result = execute(step)  → call LLM or tool
  3. kv.put(step_key, result) → persist for future restarts
  4. return result
```

### Two KV Buckets

**`AGENT_PROMISES`** — full run state, checkpointed after each LLM turn:

```
key:   {tenant_id}.{promise_id}
value: {
    id:            String,              // SHA-256(tenant_id + nats_stream_seq)
    tenant_id:     String,
    automation_id: String,
    status:        Running | Resolved | Failed,
    messages:      Vec<Message>,        // conversation history checkpoint
    iteration:     u32,
    worker_id:     String,              // which process claimed this run
    claimed_at:    DateTime<Utc>,
    trigger:       serde_json::Value,   // original event payload
}
```

**`AGENT_TOOL_RESULTS`** — cached result per tool call:

```
key:   {tenant_id}.{promise_id}.{tool_use_id}
value: serialized tool result
```

The `promise_id` is derived deterministically from `(tenant_id, NATS stream sequence number)`.
When JetStream redelivers a message, the agent finds the existing promise and continues rather
than starting fresh.

### Run Lifecycle

```
1. NATS message arrives
   → promise_id = SHA-256(tenant_id + stream_seq)
   → create AgentPromise in KV (status: Running, claimed_at: now)
   → do NOT ACK yet
   → background task: send AckKind::Progress every 15s to prevent spurious redelivery

2. Each LLM iteration:
   → call Anthropic API
   → checkpoint: kv.update(promise_key, {messages, iteration}) with CAS revision check

3. Each tool call:
   → check AGENT_TOOL_RESULTS[promise_id.tool_use_id]
   → if cached: return result (no re-execution)
   → if not: execute → save to KV → return

4. Run completes:
   → mark promise Resolved
   → msg.ack()
   → KV TTL handles cleanup automatically

5. On process startup:
   → scan AGENT_PROMISES for Running promises with claimed_at > 5 minutes
   → resume each from its checkpointed messages and iteration
```

### Failure Mode Coverage

| Scenario | Outcome |
|---|---|
| Crash during LLM call | NATS redelivers → promise found → resume from checkpoint |
| Crash after tool executes, before KV write | Tool re-executes — mitigated by idempotency keys on write APIs |
| `ack_wait` exceeded during active run | Progress heartbeats keep message in-flight — no spurious redelivery |
| Two instances race on same redelivered message | CAS on KV revision — only one wins the claim |
| Restart before NATS redelivers | Startup recovery finds stale promise and resumes proactively |

### Limitation 1: Non-Atomic Window Between Tool Execution and KV Write

The window between `execute(tool)` and `kv.put(result)` is non-atomic (milliseconds). A crash
here causes the tool to re-execute on the next attempt.

**Resolution: idempotency keys on all write tool calls**

Every write tool call includes a stable idempotency key derived from `{promise_id}.{tool_use_id}`.
On a second execution with the same key, the external API returns the original result without
performing the action again:

- **GitHub** — supports `Idempotency-Key` header on REST API mutations. A duplicate
  `post_pr_comment` with the same key returns the existing comment without creating a new one.
- **Linear** — supports `Idempotency-Key` header on GraphQL mutations.
- **Slack** — no native idempotency key support. Mitigation: before sending, check whether a
  recent message from the bot with identical content already exists in the channel. If found, skip.

With idempotency keys, the consequence of a crash in the non-atomic window changes from "action
duplicated" to "extra API call that returns the already-known result" — safe in all cases.

### Limitation 2: NATS KV Durability on OS Crash

With default NATS configuration, `fsync` runs every 2 minutes. A KV write acknowledged by NATS
but not yet fsynced can be lost on a kernel panic or power failure. Process crashes (OOM kill,
SIGKILL, pod eviction) are not affected — only OS-level failures.

**Resolution: `sync_always: true` on the `AGENT_PROMISES` bucket**

```rust
kv::Config {
    bucket: "AGENT_PROMISES",
    sync_always: true,  // fsync before each ack
    ..Default::default()
}
```

This ensures every checkpoint is on disk before NATS acknowledges the write. The throughput cost
is negligible in this context: KV writes are sub-millisecond while LLM calls take 5–30 seconds
each — the KV write is never the bottleneck.

OS crashes are rare in cloud environments. The short-term posture is to accept the risk for
process crashes (handled) and document the OS crash exposure. `sync_always: true` is the
definitive fix and should be applied in the same implementation.

**Long-term path:** Restate's journal (RocksDB with WAL) eliminates this class of problem by
design, without requiring any application-level configuration.

---

## Implementation Scope

This applies only to `trogon-agent`. Other runners were evaluated and excluded:

- **`acp/runner`** — interactive request-reply (ACP client waits and can retry); full session
  history already persisted in NATS KV via `ACP_SESSIONS`. Different durability profile.
- **`feat/xai-runner`** — tool-call iteration runs server-side in xAI's Responses API.
- **`feat/codex-runner`** — delegates the loop to the Codex CLI subprocess.
- **`feat/wasm-runtime`** — WASM execution sandbox, not an agent loop.
- **Slack / Discord / Telegram** — single-shot message responders.
- **`nats-http-detokenization-proxy`** — stateless HTTP proxy, inherently idempotent.

## Files to Change

| File | Change |
|---|---|
| `crates/trogon-agent/src/promise_store.rs` | New — `PromiseRepository` trait, NATS KV impl, in-memory mock |
| `crates/trogon-agent/src/agent_loop.rs` | Checkpoint after each LLM turn; tool result cache before dispatch |
| `crates/trogon-agent/src/runner.rs` | Create promise before run; Progress heartbeat; startup recovery |
| `crates/trogon-agent/src/lib.rs` | Export `promise_store` module |
| `crates/trogon-agent/Cargo.toml` | No new dependencies |
