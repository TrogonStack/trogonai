# A2A per-Account JetStream assets

Operator reference for the JetStream streams and KV bucket the A2A-over-NATS binding expects **inside each tenant NATS Account**. Tenancy is one Account per tenant; asset names are identical across Accounts because the Account namespace is the isolation boundary.

## Related documents

| Document | Purpose |
|----------|---------|
| [A2A plan](../A2A_PLAN.md) | Subject topology, streaming semantics, phased delivery |
| [A2A pending decisions](../A2A_PENDING_DECISION.md) | Landed decisions: stream topology, retention, push DLQ |
| [A2A NSC account bootstrap](A2A_NSC_ACCOUNT_BOOTSTRAP.md) | Full operator runbook: NSC hierarchy, User ACL templates, bootstrap order |
| [Push DLQ ops](A2A_PUSH_DLQ_OPS.md) | Stream verification, consumption, envelope schema |
| [Auth callout sketch](A2A_AUTH_CALLOUT_SKETCH.md) | Minted User JWT + stable **`caller_id`** (DLQ + ACL alignment) |
| [Gateway roadmap](A2A_GATEWAY_ROADMAP.md) | Ingress futures; not a JetStream asset owner |
| [Subject ACL quick ref](A2A_SUBJECT_ACL_QUICKREF.md) | Caller / Gateway / Registrar publish-subscribe one-pager |
| [Documentation index](A2A_DOCS_INDEX.md) | Hub linking operator and design docs |
| [AgentCard catalog — KV watch](catalog-kv-watch.md) | Client-side KV watch pattern for push-driven discovery |

---

## Assets to provision per Account

Each tenant Account needs its own copies of the assets below. Examples use the in-tree default prefix `a2a`; if the tenant uses a custom prefix, substitute consistently (stream names derive from the uppercased prefix — e.g. `MYAPP_EVENTS` for prefix `myapp`).

### `A2A_EVENTS` — task event stream

**Kind:** JetStream stream  
**Name:** `A2A_EVENTS` (with default prefix `a2a`)  
**Subject filter:** `{prefix}.task.*.events.*` — e.g. `a2a.task.*.events.*`

This is the shared per-Account stream for all task event traffic. It backs `message/stream` delivery and `tasks/resubscribe` replay. One stream per Account with subject filtering is the landed topology (not per-task streams).

**In-tree reference:** stream config and naming live under `rsworkspace/crates/a2a-nats/src/jetstream/` (`streams.rs`, `provision.rs`) and `rsworkspace/crates/a2a-nats/src/nats/subjects/stream.rs` (`A2aStream::Events`).

**Operator defaults** (from [landed decisions](../A2A_PENDING_DECISION.md)):

| Setting | Target value | Notes |
|---------|--------------|-------|
| `retention` | `interest` | Events drop when no active consumer interest |
| `discard` | `old` | Slow consumers do not block agents; oldest events drop within the retention window |
| `max_age` | `24h` | Baseline replay/resubscribe window; per-Account override for longer audit needs |
| `storage` | `file` | Matches in-tree provisioner |

The in-tree `provision_streams` helper sets **`retention = interest`**, **`discard = old`**, and **`max_age = 24h`** (override via **`A2A_EVENTS_MAX_AGE_SECS`** on agent startup).

---

### `A2A_AGENT_CARDS` — AgentCard catalog KV

**Kind:** JetStream KV bucket  
**Name:** `A2A_AGENT_CARDS` (constant in `rsworkspace/crates/a2a-nats/src/catalog/nats_kv.rs`)  
**Keys:** `{agent_id}` — one key per registered agent

Stores semi-static AgentCard payloads for discovery. The `a2a-nats-discovery` registrar is the intended writer; gateway and callers read via KV get, KV watch, or `{prefix}.discover.{agent_id}` request/reply.

**Operator defaults:**

| Setting | Value | Notes |
|---------|-------|-------|
| `history` | `1` | Only the latest revision per key is retained — sufficient for catalog use |
| `max_value_size` | `65536` | Covers typical AgentCard payloads |

For client-side watch ergonomics, see [catalog-kv-watch.md](catalog-kv-watch.md).

---

### `A2A_PUSH_DLQ` — push dead-letter stream

**Kind:** JetStream stream  
**Name:** `A2A_PUSH_DLQ`  
**Subject shape:** `{prefix}.push.dlq.{caller_id}.{task_id}` — e.g. `a2a.push.dlq.{caller_id}.{task_id}`

Per-Account DLQ for terminal push delivery failures. After HTTPS in-process retries exhaust (`HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS`), the agent **`Bridge`** publishes a structured JSON envelope (see **[push DLQ operator playbook](A2A_PUSH_DLQ_OPS.md)**) onto `{prefix}.push.dlq.{caller_id}.{task_id}` via JetStream. The `{caller_id}` segment defaults to **`_`** (`Config::push_dlq_caller_segment` / **`DEFAULT_PUSH_DLQ_CALLER_SEGMENT`**) until auth-callout or gateway propagation supplies a minted caller identity token-safe segment. Operators consume the DLQ to remediate.

**Provisioning & publish status:** `provision_streams` creates `A2A_PUSH_DLQ` alongside `A2A_EVENTS`. Agent-side publish is active on the `message/stream` terminal push path (`a2a-nats`). The **`a2a-gateway`** forwarder does not emit DLQ messages (see **`rsworkspace/crates/a2a-gateway/src/lib.rs`** crate-level Rustdoc).

In-tree subject filter: `{prefix}.push.dlq.*.*` (wildcard tokens match `caller_id`, `task_id`). **Operator check:** when `a2a-nats` provisions this stream, verify `nats stream info A2A_PUSH_DLQ` shows that filter (e.g. `a2a.push.dlq.*.*` with the default prefix).

---

## Not provisioning automation

This repository does **not** ship turnkey infrastructure-as-code or a single command that provisions a complete tenant Account end-to-end.

Operators create JetStream streams and KV buckets using their platform's standard tooling:

- **`nats` CLI** — [`nats stream add`](https://docs.nats.io/using-nats/nats-tools/nsc/stream), [`nats kv add`](https://docs.nats.io/nats-concepts/jetstream/key-value-store/kv_walkthrough)
- **Terraform / Pulumi** — NATS provider or JetStream management API
- **Platform JetStream admin** — org-specific controllers, Helm charts, etc.

`nsc` configures Operator/Account/User JWTs and permissions; it does **not** create JetStream assets. After the Account JWT is pushed to the cluster and JetStream is enabled on the Account, create the assets above with an Account admin User.

Optional in-tree helpers (not a substitute for operator-owned bootstrap):

- `a2a-nats` — `provision_streams` creates `A2A_EVENTS` and `A2A_PUSH_DLQ` per Account. **Push DLQ JSON publishes ship** from `message/stream` terminal push dispatch when delivery fails (`push/dlq.rs`); **`a2a-gateway`** ingress does not own push/DLQ (see gateway crate README / `lib.rs`).
- `a2a-nats-discovery` — provisions the `A2A_AGENT_CARDS` KV bucket on startup

Official references:

- [JetStream — streams](https://docs.nats.io/nats-concepts/jetstream/streams)
- [JetStream — key-value store](https://docs.nats.io/nats-concepts/jetstream/key-value-store)
- [Configuring JetStream — account resource limits](https://docs.nats.io/running-a-nats-service/configuration/resource_management)

For NSC JWT hierarchy, User ACL templates, and step-by-step bootstrap order, use [A2A_NSC_ACCOUNT_BOOTSTRAP.md](A2A_NSC_ACCOUNT_BOOTSTRAP.md).

---

## Verification checklist

Run these checks after bootstrap, connected with a User that has JetStream management read access inside the tenant Account.

### Stream `A2A_EVENTS`

- [ ] Stream exists: `nats stream info A2A_EVENTS`
- [ ] Subject filter includes `{prefix}.task.*.events.*` (e.g. `a2a.task.*.events.*`)
- [ ] Retention is `interest` (or documented tenant override); discard is `old`
- [ ] `max_age` matches tenant policy (default 24h)
- [ ] Storage is `file`

### KV bucket `A2A_AGENT_CARDS`

- [ ] Bucket reachable: `nats kv info A2A_AGENT_CARDS`
- [ ] `history = 1`; `max_value_size` ≥ 65536
- [ ] Test put/get with a registrar-scoped User succeeds; caller Users cannot write

### Stream `A2A_PUSH_DLQ` (when provisioned)

- [ ] Stream exists: `nats stream info A2A_PUSH_DLQ`
- [ ] Subject filter covers `{prefix}.push.dlq.*.*` (or equivalent `{prefix}.push.dlq.>` wildcard) for `{caller_id}.{task_id}` segments
- [ ] Acknowledge DLQ publishing is **not yet active** in the gateway — no messages expected until cross-process DLQ ships

### End-to-end smoke (optional)

- [ ] Publish a test message on `{prefix}.task.{task_id}.events.{req_id}` and confirm it lands in `A2A_EVENTS`
- [ ] Registrar can write an AgentCard key; gateway/caller can read it via KV or `{prefix}.discover.{agent_id}`
