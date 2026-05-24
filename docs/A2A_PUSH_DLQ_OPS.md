# A2A push DLQ — operator playbook

Short runbook for the per-Account JetStream dead-letter stream used when terminal push notification delivery fails.

## Related documents

| Document | Purpose |
|----------|---------|
| [A2A plan](../A2A_PLAN.md) | Push delivery semantics, Bridge dispatch |
| [A2A TODO](../A2A_TODO.md) | Remaining phased work (gateway, exactly-once, …) |
| [Push exactly-once sketch](./A2A_PUSH_EXACTLY_ONCE_SKETCH.md) | Opt-in idempotency keys, JetStream dedupe, HTTP cooperation, `subject:` best-effort |
| [A2A per-Account JetStream assets](./A2A_JETSTREAM_ACCOUNT_STREAMS.md) | Stream provisioning reference alongside **`A2A_EVENTS`** |
| [Auth callout sketch](./A2A_AUTH_CALLOUT_SKETCH.md) | Minted User JWT → stable **`caller_id`** for ACL + DLQ segments |
| [Gateway roadmap](./A2A_GATEWAY_ROADMAP.md) | Ingress futures (JWT, Tier 1, ingress audit); gateway does **not** emit DLQ today |

---

## Stream identity

Each tenant NATS Account owns one push DLQ stream. The in-tree default prefix is `a2a`; substitute your tenant prefix consistently.

| Field | Value |
|-------|-------|
| **Stream name** | **`{PREFIX}_PUSH_DLQ`** — e.g. **`A2A_PUSH_DLQ`** when prefix is `a2a` (prefix uppercased, dots → underscores) |
| **Subject filter** | **`{prefix}.push.dlq.*.*`** — e.g. **`a2a.push.dlq.*.*`** |
| **Publish subject** | **`{prefix}.push.dlq.{caller_id}.{task_id}`** |

---

## Verify the stream exists

Connect with a User that has JetStream management read access inside the tenant Account, then inspect the stream:

```bash
# Replace TENANT with your nats CLI context (server URL, credentials, account).
nats --context TENANT stream info A2A_PUSH_DLQ
```

Confirm the output includes:

- Stream name **`{PREFIX}_PUSH_DLQ`** (default: **`A2A_PUSH_DLQ`**)
- Subject filter **`{prefix}.push.dlq.*.*`** (default: **`a2a.push.dlq.*.*`**)

If the stream is missing, provision it during Account bootstrap — see [A2A per-Account JetStream assets](./A2A_JETSTREAM_ACCOUNT_STREAMS.md) or run in-tree **`provision_streams`** from **`a2a-nats`.

---

## Consume DLQ messages

Create a durable pull consumer (or use an ephemeral sub for ad-hoc inspection):

```bash
# Durable consumer on the tenant stream (adjust filter if using a custom prefix).
nats --context TENANT consumer add A2A_PUSH_DLQ PUSH_DLQ_OPS \
  --filter "a2a.push.dlq.>" \
  --ack explicit \
  --deliver all \
  --replay instant

# Read and ack messages interactively.
nats --context TENANT consumer sub A2A_PUSH_DLQ PUSH_DLQ_OPS
```

For one-off sampling without a durable consumer:

```bash
nats --context TENANT sub "a2a.push.dlq.>" --stream A2A_PUSH_DLQ
```

Grant the ops User **`--allow-sub`** on **`{prefix}.push.dlq.>`** (or tighter **`{prefix}.push.dlq.*.*`**) and JetStream consumer management on **`A2A_PUSH_DLQ`**.

---

## Message payload

Failed push entries are **JSON** messages published by the agent **`Bridge`** when push dispatch fails after in-process retries are exhausted (HTTPS webhooks retry transient failures up to **`HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS`**; NATS/JetStream target failures are recorded without HTTP-style retry where not applicable).

Minimal envelope skeleton:

```json
{
  "schema": "a2a.push.dlq/v1",
  "task_id": "<task-uuid>",
  "push_config_id": "<push-notification-config-id>",
  "target_url": "https://example.com/webhook",
  "error": "HTTP request failed: 503 Service Unavailable",
  "notification": { }
}
```

| Key | Type | Notes |
|-----|------|-------|
| `schema` | string | Version tag, e.g. **`a2a.push.dlq/v1`** |
| `task_id` | string | A2A task identifier |
| `push_config_id` | string | **`tasks/pushNotificationConfig`** entry that failed |
| `target_url` | string | Resolved push target (HTTP URL, **`subject:…`**, or **`jetstream:…`**) |
| `error` | string | Terminal failure reason from the dispatcher |
| `notification` | JSON value **or** string | Original notification body when JSON; fallback string when serialization fails |

Use DLQ messages for remediation: fix webhook endpoints, replay after config correction, or alert on sustained failure rates per **`caller_id`** / **`task_id`** subject segments.

---

## Caller ID segment

The `{caller_id}` token in **`{prefix}.push.dlq.{caller_id}.{task_id}`** identifies which external caller triggered the failed push.

| Source | `{caller_id}` value |
|--------|---------------------|
| **No principal** (default today — auth-callout not deployed, direct agent connect) | **`_`** (`DEFAULT_PUSH_DLQ_CALLER_SEGMENT`), or **`A2A_PUSH_DLQ_CALLER_SEGMENT`** / **`Config::with_push_dlq_caller_segment`** when set explicitly |
| **Principal present** via NATS header **`X-A2a-Spicedb-Principal`** (JSON User JWT `data` claim forwarded by gateway ingress once auth-callout deploys) | Sanitized **`spicedb_subject`** from the principal payload (`.` and spaces → `_`) |
| **Principal present but `spicedb_subject` absent** | Falls back to **`_`** (or env override); agent logs a structured warning |

Gateway-side DLQ mirror preserves whatever segment the agent published.

---

## Embedder configuration

Agents using **`Bridge`** run **`apply_timeout_overrides`** from **`a2a-nats`** (see **`a2a-nats-agent`**, **`a2a-nats-stdio`**, **`a2a-nats-server`**). Set the DLQ **`{caller_id}`** fallback segment via:

| Mechanism | Notes |
|-----------|-------|
| **`A2A_PUSH_DLQ_CALLER_SEGMENT`** | Fallback when no gateway principal arrives; whitespace-only values fall back to **`_`**. Superseded per-request when **`X-A2a-Spicedb-Principal`** carries a principal with **`spicedb_subject`** ([auth callout sketch](./A2A_AUTH_CALLOUT_SKETCH.md)). |
| **`A2A_MAX_CONCURRENT_CLIENT_TASKS`** | Caps concurrent **`Bridge`** streaming tasks (**`Semaphore`**). **`0`** normalizes to **`1`**. Invalid integers are ignored with a warning. |
| **`Config::with_push_dlq_caller_segment`** | Programmatic override when constructing **`Config`**. |

---

## Operational notes

- DLQ is **per-Account** — cross-tenant isolation matches other A2A JetStream assets.
- **`a2a-gateway`** forwarder does **not** emit agent-origin DLQ traffic; optional **`A2A_GATEWAY_PUSH_DLQ_MIRROR`** republishes agent envelopes to **`{prefix}.push.dlq.mirror.*`** for ops visibility (see **Gateway mirror mode** above). Terminal DLQ JSON is published from **`a2a-nats`** **`message/stream`** on the agent **`Bridge`**.
- After remediation, ack processed DLQ messages so retention/discards do not hide new failures.

---

## Gateway mirror mode (optional)

When **`A2A_GATEWAY_PUSH_DLQ_MIRROR=on`**, the **`a2a-gateway`** process runs a background JetStream pull consumer on **`{prefix}.push.dlq.>`** and republishes each agent-origin DLQ envelope onto **`{prefix}.push.dlq.mirror.{caller_id}.{task_id}`** in the same **`A2A_PUSH_DLQ`** stream. Mirror copies carry header **`X-A2a-Dlq-Mirrored: true`**; the consumer skips subjects that already contain **`.mirror.`** or carry that header so the loop cannot recurse.

| Variable | Default | Meaning |
|----------|---------|---------|
| **`A2A_GATEWAY_PUSH_DLQ_MIRROR`** | off | Set to **`on`**, **`true`**, **`1`**, or **`yes`** to enable the mirror task |
| **`A2A_GATEWAY_PUSH_DLQ_DURABLE`** | **`a2a-gateway-push-dlq-mirror`** | Override the durable pull consumer name |

The gateway User must have JetStream **read** on **`A2A_PUSH_DLQ`** (consumer + get) and **publish** on **`{prefix}.push.dlq.mirror.*.*`**. Existing streams provisioned before mirror support may need their subject filter extended with **`{prefix}.push.dlq.mirror.*.*`** (in-tree **`provision_streams`** includes both patterns).

Mirror mode is for operator visibility alongside agent publishes — it does **not** replace agent-side terminal DLQ emission or provide exactly-once deduplication across agent + mirror paths.
