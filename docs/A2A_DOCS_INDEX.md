# A2A over NATS — documentation index

Central navigation for the A2A-over-NATS binding: architecture, operator runbooks, gateway/auth design, and developer ergonomics.

## Architecture & tracker

| Document | Purpose |
|----------|---------|
| [`../A2A_PLAN.md`](../A2A_PLAN.md) | Master architecture — subject topology, streaming semantics, phased delivery, implementation status |
| [`../A2A_TODO.md`](../A2A_TODO.md) | Engineering work tracker — gaps between plan and in-tree code |
| [`../A2A_PENDING_DECISION.md`](../A2A_PENDING_DECISION.md) | Landed architectural decisions and audit trail for future choices |

## Operators

| Document | Purpose |
|----------|---------|
| [`./A2A_NSC_ACCOUNT_BOOTSTRAP.md`](./A2A_NSC_ACCOUNT_BOOTSTRAP.md) | Per-tenant NATS Account provisioning — NSC bootstrap, ACL templates, JetStream/KV setup |
| [`./A2A_JETSTREAM_ACCOUNT_STREAMS.md`](./A2A_JETSTREAM_ACCOUNT_STREAMS.md) | JetStream streams and KV bucket reference inside each tenant Account |
| [`./A2A_SUBJECT_ACL_QUICKREF.md`](./A2A_SUBJECT_ACL_QUICKREF.md) | One-page Phase 0 subject ACL summary for caller, gateway, and registrar Users |
| [`./A2A_PUSH_DLQ_OPS.md`](./A2A_PUSH_DLQ_OPS.md) | Push notification dead-letter stream — triage, replay, and operational playbook |
| [`./A2A_PUSH_EXACTLY_ONCE_SKETCH.md`](./A2A_PUSH_EXACTLY_ONCE_SKETCH.md) | Opt-in exactly-once push delivery — idempotency keys, per-target semantics, DLQ interaction |
| [`./A2A_STREAMING_BACKPRESSURE_OPS.md`](./A2A_STREAMING_BACKPRESSURE_OPS.md) | Task event stream back-pressure — `A2A_EVENTS` policy, gateway pull consumers, agent `Bridge` limits |
| [`./A2A_RUNTIME_ENV.md`](./A2A_RUNTIME_ENV.md) | Consolidated env vars and CLI flags for A2A binaries and `a2a_nats` embedders |

## Gateway & auth

| Document | Purpose |
|----------|---------|
| [`./A2A_GATEWAY_ROADMAP.md`](./A2A_GATEWAY_ROADMAP.md) | `a2a-gateway` engineering checklist — ingress policy, audit, and phased delivery |
| [`./A2A_AUTH_CALLOUT_SKETCH.md`](./A2A_AUTH_CALLOUT_SKETCH.md) | Auth callout service sketch — scoped JWT minting and AgentCard scheme wiring |
| [`./A2A_BRIDGE_SKETCH.md`](./A2A_BRIDGE_SKETCH.md) | Future `a2a-bridge` HTTPS sidecar — perimeter placement, auth re-mint, SSE↔JetStream mapping |
| [`./A2A_FEDERATED_DISCOVERY_SKETCH.md`](./A2A_FEDERATED_DISCOVERY_SKETCH.md) | Phase 4 cross-Account discovery — operator export/import of `{prefix}.discover.>`, trust boundaries, SpiceDB at federation boundary |

## Operator tooling

| Resource | Purpose |
|----------|---------|
| [`../scripts/a2a-nsc-bootstrap.sh`](../scripts/a2a-nsc-bootstrap.sh) | Idempotent shell script — provisions one tenant Account with three service Users (caller-template, gateway, registrar) and Phase 0 ACLs. Reads ACL subjects from `scripts/acl-templates/`. |
| [`../scripts/acl-templates/caller.acl`](../scripts/acl-templates/caller.acl) | Caller User subject ACL template (allow-pub / allow-sub / deny-pub) |
| [`../scripts/acl-templates/gateway.acl`](../scripts/acl-templates/gateway.acl) | Gateway User subject ACL template |
| [`../scripts/acl-templates/registrar.acl`](../scripts/acl-templates/registrar.acl) | Registrar service User subject ACL template |
| [`../scripts/a2a-nsc-export-discovery.sh`](../scripts/a2a-nsc-export-discovery.sh) | Phase 4 federated discovery export/import scaffold — wraps `nsc add export` / `nsc add import` for `{prefix}.discover.>`; supports `--dry-run` and `--import` flags. |

## Developer ergonomics

| Document | Purpose |
|----------|---------|
| [`./A2A_DEVELOPMENT.md`](./A2A_DEVELOPMENT.md) | Local workspace setup, `cargo test` / `cargo clippy`, and key crate map |
| [`./catalog-kv-watch.md`](./catalog-kv-watch.md) | Push-driven AgentCard catalog freshness via JetStream KV watch |
