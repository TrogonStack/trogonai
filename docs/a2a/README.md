# A2A over NATS — documentation

NATS-native binding for the [Agent2Agent (A2A) protocol](https://a2aproject.github.io/A2A/). Pick a section based on what you need to do.

## Learning

New to the binding or setting up locally?

- [Developer guide](./tutorials/development.md) — workspace setup, `cargo`/`mise` workflow, local docker compose smoke harness (`make smoke` / `make smoke-full`).

## Tasks

You know the binding; you need to accomplish something.

### Operators

- [NSC account bootstrap](./how-to/operators/nsc-account-bootstrap.md) — per-tenant NATS Account, service Users, ACL templates, JetStream/KV provisioning.
- [Auth-callout deployment](./how-to/operators/auth-callout-deployment.md) — running `a2a-auth-callout` on `$SYS.REQ.USER.AUTH`, signing-key custody.
- [Push DLQ triage](./how-to/operators/push-dlq-triage.md) — terminal push failure inspection, replay, retention.
- [Streaming back-pressure](./how-to/operators/streaming-backpressure.md) — `A2A_EVENTS` JetStream policy, agent `Bridge` limits, gateway pull consumers.

### Catalog

- [KV watch (push discovery)](./how-to/catalog/kv-watch.md) — push-driven AgentCard freshness instead of polling `DiscoverService`.

## Reference

Look up exact shapes and values.

- [Runtime environment](./reference/runtime-env.md) — env vars and CLI flags across `a2a-*` binaries and `a2a_nats` embedders.
- [Subject ACL quickref](./reference/subject-acl-quickref.md) — one-page Phase 0 ACL summary for caller / gateway / registrar Users.
- [JetStream account assets](./reference/jetstream-account-streams.md) — streams and KV buckets expected inside each tenant Account.
- [Policy](./reference/policy/) — gateway ingress policy tiers:
  - [Tier-1 declarative](./reference/policy/tier1-declarative.md)
  - [Tier-2 CEL](./reference/policy/tier2-cel.md)
  - [Tier-3 redaction](./reference/policy/tier3-redaction.md)
  - [Tier-3 skills catalog](./reference/policy/tier3-skills-catalog.md)

## Understanding

Why the binding looks the way it does, where it's heading.

- [Architecture](./explanation/architecture.md) — subject topology, streaming semantics, auth model, policy tiers, audit envelope, landed decisions.
- [Auth-callout design](./explanation/auth-callout-design.md) — wire format and mint contract on the NATS perimeter.
- [Gateway roadmap](./explanation/gateway-roadmap.md) — engineering checklist for `a2a-gateway` beyond opaque forward.
- [Bridge sketch](./explanation/bridge-sketch.md) — future `a2a-bridge` HTTPS↔NATS sidecar shape.
- [Federated discovery sketch](./explanation/federated-discovery-sketch.md) — Phase 4 cross-Account AgentCard discovery.
- [Push exactly-once sketch](./explanation/push-exactly-once-sketch.md) — opt-in exactly-once terminal push delivery design.

## Operator tooling

- [`scripts/a2a-nsc-bootstrap.sh`](../../scripts/a2a-nsc-bootstrap.sh) — idempotent Account provisioning script.
- [`scripts/acl-templates/`](../../scripts/acl-templates/) — caller / gateway / registrar User ACL templates.
- [`scripts/a2a-nsc-export-discovery.sh`](../../scripts/a2a-nsc-export-discovery.sh) — Phase 4 federated discovery export/import scaffold.
