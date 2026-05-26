# A2A push exactly-once — design sketch

Engineering sketch for opt-in **exactly-once** terminal push delivery (Phase 3). Not implemented in-tree yet; tracked in [A2A architecture](architecture.md) §Phase 3 and decided in [A2A architecture](architecture.md) §Decisions.

## Related links

| Document | Purpose |
|----------|---------|
| [Push DLQ triage](../how-to/operators/push-dlq-triage.md) | Terminal push failure path — **`push::dlq`** publishes to **`A2A_PUSH_DLQ`** after retries exhaust |
| [A2A architecture](architecture.md) | Open engineering items (exactly-once flag, gateway DLQ mirroring, …) |
| [A2A architecture](architecture.md) | Push target schemes (`http(s)://`, `subject:`, `jetstream:`) and default at-least-once semantics |

---

## Motivation

Today the agent **`Bridge`** dispatches push notifications **once per terminal `StreamResponse`** from the `message/stream` event pump (`message_stream.rs`). Delivery is **at-least-once by default**:

- In-process retries (`HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS` = 3) on HTTPS, core NATS (`subject:`), and JetStream (`jetstream:`) targets can re-send the same notification body after a transient failure.
- A successful first attempt followed by a late retry (e.g. slow HTTP response, ambiguous NATS error) can produce **duplicate deliveries** at the consumer.
- There is no idempotency contract on the outbound envelope; receivers must dedupe themselves if they care.

Phase 3 adds an **opt-in flag on `PushNotificationConfig`** (name TBD — e.g. `exactlyOnceDelivery: true`) so callers who need single-effect delivery can enable agent-side dedupe + transport-specific idempotency keys. **When the flag is absent or false, behavior stays exactly as shipped today (at-least-once).**

---

## Scope and non-goals

| In scope | Out of scope |
|----------|--------------|
| Terminal-status push only (same trigger as today) | Mid-task / non-terminal push |
| Per-`PushNotificationConfig` opt-in | Global account-wide exactly-once default |
| Idempotency key generation + transport mapping | Receiver-side idempotency store (caller's responsibility) |
| JetStream `Nats-Msg-Id` dedupe for `jetstream:` targets | Changing **`A2A_EVENTS`** retention or stream topology |
| HTTP `Idempotency-Key` header for webhooks | Digest auth (deferred per plan) |

---

## Idempotency key — derivation and placement

### Derivation (agent-side)

Stable key for a single logical terminal notification:

```
{task_id}:{push_config_id}:{terminal_task_state}
```

- **`task_id`** — A2A task identifier.
- **`push_config_id`** — `TaskPushNotificationConfig.id` from `tasks/pushNotificationConfig/set`.
- **`terminal_task_state`** — enum name for the terminal transition (`completed`, `failed`, `canceled`, `rejected`).

One task may register multiple push configs; each config gets its own key suffix. Re-dispatch of the same terminal event for the same config (retries, bridge restart) must reuse the **same** key.

Optional future extension: include a content hash of the serialized `StreamResponse` if redaction or payload drift must participate in dedupe — not required for v1.

### Where the key lives on the wire

| Surface | Role |
|---------|------|
| **Transport headers (primary)** | Canonical carrier for dedupe at the messaging / HTTP layer. |
| **JSON body (secondary, optional)** | A2A extension field for receivers that only inspect the SSE-shaped envelope. |

**Headers (always set when exactly-once is enabled):**

| Target | Header | Value |
|--------|--------|-------|
| HTTPS webhook | `Idempotency-Key` | Derived key (UTF-8) |
| Core NATS `subject:` | `Nats-Msg-Id` | Derived key — best-effort only (see below) |
| JetStream `jetstream:` | `Nats-Msg-Id` | Derived key — JetStream dedupe window applies |

**JSON body (optional extension):**

When exactly-once is enabled, the agent may add a top-level sibling field on the outbound notification object (exact name TBD, e.g. `_a2aPushIdempotencyKey`) mirroring the header value. Receivers should prefer the header when both are present. Default at-least-once mode omits the field.

The notification body remains the standard A2A `StreamResponse` JSON (same as SSE); the extension must not alter A2A-visible semantics for clients that ignore it.

---

## Per-target delivery semantics

### `jetstream:` — strongest in-binding guarantee

JetStream supports publish-time deduplication via the **`Nats-Msg-Id`** header within the stream's **`duplicate_window`** (server default typically two minutes; operator-configurable on the caller-provisioned stream).

**Exactly-once mode:**

1. Set `Nats-Msg-Id` to the derived idempotency key on every publish attempt (including retries).
2. Wait for JetStream publish ack before treating delivery as successful.
3. On duplicate within the window, JetStream returns a duplicate ack — agent treats as **success** (no DLQ, no further retries).

**Implications:**

- Callers must provision the target stream with a `duplicate_window` wide enough to cover agent retry backoff (3 attempts, max ~3s backoff cap today) plus clock skew.
- Dedupe is **window-bound**, not infinite — replays after window expiry are new publishes (document for operators).
- "Double-ack" here means: **JetStream publish ack** (agent confirms JS accepted the message) + **downstream consumer ack** (caller's problem — pull consumer explicit ack, etc.).

### `https://` / `http://` — cooperative exactly-once

HTTP has no broker-level dedupe. Exactly-once is **cooperative**:

1. Agent sends `Idempotency-Key: {derived key}` on every attempt (including retries within `HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS`).
2. Receiver **must** treat repeated keys as no-ops (return `2xx`, same response shape) within a retention window it defines.
3. Agent treats any `2xx` as success; only terminal non-retryable failures or exhausted retries → **`push::dlq`**.

**Duplicate risk on retry (today and with exactly-once flag):**

| Scenario | At-least-once (default) | Exactly-once (opt-in) |
|----------|-------------------------|------------------------|
| Attempt 1 succeeds at receiver; agent times out before reading response | Duplicate on retry | Same — receiver must dedupe on `Idempotency-Key` |
| Attempt 1 returns `503`; agent retries | Duplicate possible if first attempt actually processed | Receiver dedupes on key |
| All 3 attempts fail | Single DLQ entry (no successful delivery) | Same |

The flag does **not** eliminate ambiguous HTTP outcomes; it gives receivers a stable key to implement idempotency. Without receiver cooperation, behavior degrades to at-least-once.

### `subject:` (core NATS) — best effort only

Core NATS pub/sub has **no** server-side deduplication. `Nats-Msg-Id` on a plain publish is informational only — subscribers may see duplicates from:

- Agent in-process retries (`NatsPublishPushDispatcher` uses the same 3-attempt loop as HTTPS/JS).
- Multiple agent replicas if queue semantics are not used on the target subject.
- Network duplicates outside NATS.

**Exactly-once mode (honest contract):**

- **Single publish attempt** — disable in-process retries when the flag is set; one `publish_with_headers`, then success or terminal failure.
- Set `Nats-Msg-Id` header for observability and for consumers that choose to dedupe in application code.
- On failure → **`push::dlq`** (same terminal path as today).
- Document as **"best-effort single publish"**, not true exactly-once. Callers needing dedupe should use `jetstream:` targets or HTTPS with receiver idempotency.

---

## Interaction with `push::dlq` and retries

### Current terminal failure path

```
message/stream terminal StreamResponse
  → dispatch_push_notifications (list configs, dispatch each)
    → CompositePushDispatcher (HTTP | subject | jetstream)
      → up to HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS with backoff
    → on terminal Err: push::dlq::publish_push_delivery_failure → A2A_PUSH_DLQ
```

See [Push DLQ triage](../how-to/operators/push-dlq-triage.md) for envelope shape and ops consumption.

### Exactly-once interactions

| Component | Behavior |
|-----------|----------|
| **`HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS`** | Default unchanged. When exactly-once enabled: **`jetstream:`** and **`https:`** keep retries but reuse the same idempotency key; **`subject:`** skips retries (single attempt). |
| **`push::dlq`** | Still fires only after **terminal dispatch failure** — not on duplicate suppressed by JetStream dedupe (duplicate ack = success). DLQ entries should include `idempotency_key` field (v2 schema or optional v1 field) for replay tooling. |
| **Successful delivery + DLQ** | Must not happen: duplicate JS ack and HTTP `2xx` must not enqueue DLQ. |
| **Replay from DLQ** | Operator replay is a **new** delivery intent; replays should mint a new key suffix (e.g. `:replay:{seq}`) or be documented as at-least-once remediation, not exactly-once. |

### Retry-induced duplicate diagram (default mode)

```
Agent                    HTTPS webhook
  |-- POST (attempt 1) -->| 200 OK (slow)
  | (timeout / no read)   |
  |-- POST (attempt 2) -->| 200 OK   ← duplicate delivery
  | OK                    |
```

With exactly-once + receiver idempotency:

```
Agent                    HTTPS webhook
  |-- POST + Idempotency-Key: K -->| processes, 200 OK (slow)
  | (timeout)                      |
  |-- POST + Idempotency-Key: K -->| recognizes K, 200 OK (no side effect)
  | OK                             |
```

---

## `PushNotificationConfig` flag (sketch)

Wire shape (illustrative — lands in `a2a-types` / A2A schema bundle):

```json
{
  "id": "cfg-1",
  "url": "jetstream:a2a.push.acme.caller-42.task-9",
  "exactlyOnceDelivery": true
}
```

| Value | Semantics |
|-------|-----------|
| **absent / `false`** | **Default — at-least-once** (current shipped behavior). |
| **`true`** | Enable idempotency key + per-target rules above. |

Validation: flag is ignored for non-terminal dispatch paths (there are none today). Invalid combinations (e.g. exactly-once + Digest auth) follow existing auth deferral rules.

---

## Implementation touchpoints (future)

| Location | Change |
|----------|--------|
| `push/dispatcher.rs` | Branch on config flag; attach headers; JS duplicate ack handling; suppress `subject:` retries |
| `agent/message_stream.rs` | Pass terminal state into dispatch for key derivation |
| `push/dlq.rs` | Optional `idempotency_key` on DLQ envelope |
| `constants.rs` | Document interaction with `HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS` |
| Gateway (future) | If gateway ever dispatches push, same flag semantics; gateway does **not** own DLQ today |

---

## Summary

| Mode | Default? | Delivery guarantee |
|------|----------|-------------------|
| **At-least-once** | **Yes** | Retries up to 3; duplicates possible; DLQ on terminal failure. |
| **Exactly-once (opt-in)** | No | **`jetstream:`** — JS `Nats-Msg-Id` dedupe within window. **`https:`** — cooperative via `Idempotency-Key`. **`subject:`** — best-effort single publish + DLQ; not true exactly-once. |

Default remains **at-least-once** until callers explicitly opt in on `PushNotificationConfig`.
