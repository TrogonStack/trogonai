# A2A subject ACL — operator quick reference

One-page summary of Phase 0 per-User NATS subject ACL inside a tenant Account. Full provisioning steps: [A2A NSC account bootstrap](./A2A_NSC_ACCOUNT_BOOTSTRAP.md).

**Related:** [A2A plan](../A2A_PLAN.md) · [A2A pending decisions](../A2A_PENDING_DECISION.md) · [A2A JetStream account streams](./A2A_JETSTREAM_ACCOUNT_STREAMS.md)

---

## Tenancy model

One **NATS Account per tenant**. Subjects carry **no `{tenant}` segment** — the Account namespace is the tenant boundary. Default prefix is `a2a`; substitute `{prefix}` consistently when using a custom `A2A_PREFIX`.

---

## Phase 0 User roles — publish / subscribe

| Role | Publish (allow) | Subscribe (allow) | Typical use |
|------|-----------------|-------------------|-------------|
| **Caller User** | `a2a.gateway.>` | `_INBOX.{caller_id}.>` | External or bridge-originated identity; request/reply into gateway ingress; replies on caller-owned inbox. Also provision subscribe on `a2a.push.{caller_id}.>` for push read path ([pending decision](../A2A_PENDING_DECISION.md)). **Do not** grant `{prefix}.catalog.register.*` publish. |
| **Gateway User** | `a2a.agent.>`, `a2a.task.>`, `a2a.push.>` | `a2a.gateway.>` | Long-lived `a2a-gateway` service; queue-group consumer on ingress; forwards to agents, task events, push envelopes. Read-only catalog (KV get/watch or `{prefix}.discover.*`); no KV put. |
| **Registrar service User** | `{prefix}.catalog.register.*` | `{prefix}.discover.*` | Long-lived `a2a-nats-discovery` identity; catalog write ingress + KV-backed discover request/reply. Grant `--allow-pub-response` on both paths. |

**Registrar publish note.** The ACL table lists registrar **subscribe** on `{prefix}.discover.*` and write ingress on `{prefix}.catalog.register.*`; deny `{prefix}.catalog.register.*` **publish** on caller and gateway Users ([bootstrap § Subject ACL templates](./A2A_NSC_ACCOUNT_BOOTSTRAP.md)).

**Caller reply subjects.** Use [`--allow-pub-response`](https://nats-io.github.io/nsc/nsc_add_user.html) when the client library needs dynamic reply subjects beyond the explicit `_INBOX.{caller_id}.>` ACL.

---

## JetStream / KV (registrar vs everyone else)

- **KV write exclusivity:** Only the **registrar service User** holds put/update on JetStream KV bucket **`A2A_AGENT_CARDS`**. Gateway and caller Users read AgentCards via `{prefix}.discover.{agent_id}` request/reply or KV get/watch — never direct catalog register publish.
- **Optional ops read:** Registrar (or a dedicated ops User) may be granted JetStream **read** on stream **`A2A_EVENTS`** (`{prefix}.task.*.events.*`) and **`A2A_PUSH_DLQ`** (`{prefix}.push.dlq.*.*`) for troubleshooting. See [JetStream account streams](./A2A_JETSTREAM_ACCOUNT_STREAMS.md) and [bootstrap § JetStream assets](./A2A_NSC_ACCOUNT_BOOTSTRAP.md).

---

## `{caller_id}`

`{caller_id}` is a **stable, token-safe segment** derived when the auth callout mints the caller User JWT — same value across reconnects and re-mints within the tenant Account. It appears in:

- `_INBOX.{caller_id}.>` (subscribe ACL)
- `a2a.push.{caller_id}.>` (push consumer read ACL)
- `{prefix}.push.dlq.{caller_id}.{task_id}` (DLQ subjects)

Details: [A2A auth callout design](./A2A_AUTH_CALLOUT_DESIGN.md).
