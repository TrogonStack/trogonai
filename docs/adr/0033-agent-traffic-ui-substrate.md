# ADR 0033: Agent-traffic UI Substrate — CLI-first via `agctl traffic`

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-29) |
| **Date** | 2026-05-29 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | `PENDING_TODO.md` Block 4 final open item ("Decide UI substrate"); unblocks the v1 acceptance for `docs/identity/agent-traffic.md` ("A security engineer can answer 'what did agent X do on behalf of user Y in the last hour, and what was blocked' in < 30 seconds"). |
| **Related** | [ADR 0030](0030-gateway-ctl-cli-surface.md) (`trogon-gateway-ctl` taxonomy and output envelope); [docs/identity/agent-traffic.md](../identity/agent-traffic.md) (v1 view spec: schema, projector, OCSF export); `rsworkspace/crates/trogon-traffic-view` (indexer + OCSF exporter); `rsworkspace/crates/agctl/src/traffic.rs` (`agctl traffic query` / `agctl traffic tail`). |

## Context

Block 4 of `PENDING_TODO.md` ships an agent-traffic view: a query-optimized projection of audit envelopes (`agent_id`, `wkl`, `purpose`, `session_id`, `act_chain`) suitable for timelines, chain exploration, top-N dashboards, and SIEM export. The schema, projector, OCSF v1 export, and Postgres indexer are accepted (`docs/identity/agent-traffic.md`) and a working skeleton lives at `rsworkspace/crates/trogon-traffic-view`.

The remaining decision is **which surface** the on-call operator and security engineer use to answer the acceptance question. Three candidates were on the table:

| Candidate | Status today | Cost to v1 |
|---|---|---|
| Reuse a `trogon-gateway` UI | No such UI exists in-tree; no Web/React workspace exists. | Bootstrap a TS workspace, design SSO, host long-lived service. |
| Standalone web console | No precedent; would duplicate `agctl` auth + envelope work. | Same as above plus duplicate output schema. |
| **CLI-first via `agctl traffic`** | `agctl traffic query` and `agctl traffic tail` already parse, connect to Postgres via `TROGON_PG_URL`, and render `table` / `--json`. ADR 0030 already pins the `trogon-gateway-ctl` taxonomy; `agctl` is the existing local-operator binary. | Zero new surfaces; close the open item by recording the decision. |

A web console is **not on the critical path** for the v1 acceptance — every question in the spec ("what did X do for Y in the last hour, what was blocked, how deep was the chain") reduces to a parametrized SQL projection that the CLI already exposes. Operators answer the same question today with `kubectl logs | jq`; replacing that with a typed `agctl traffic` invocation against the Postgres index is a strict upgrade.

## Decision

**The v1 surface for the agent-traffic view is the `agctl traffic` CLI**, backed by `trogon-traffic-view`'s Postgres indexer.

- `agctl traffic query --tenant <t> [--agent-id <a/b>] --since <dur> [--json]` answers the headline acceptance question.
- `agctl traffic tail --tenant <t> [--since <dur>] [--limit <n>] [--json]` covers the live-tail workflow.
- OCSF export remains a separate JetStream durable consumer (`trogon-traffic-view::ocsf`), not a CLI surface — SIEM systems pull from the stream, not from the CLI.
- A web console is **explicitly deferred**. It is not in scope for any block in `PENDING_TODO.md` and will require its own ADR if ever revisited; that ADR must justify the additional auth surface, the duplicated envelope schema work, and the operational cost of hosting a service that does not exist today.
- The CLI's JSON envelope is the contract: any future UI (terminal TUI, Grafana panel, web console) consumes `--json` output or queries `trogon-traffic-view::TrafficIndex` directly — it does **not** introduce a parallel REST surface.

## Consequences

**Positive**

- Closes Block 4 with zero additional surface area. The acceptance criterion ("< 30 seconds to answer") is met by a single shell invocation against an indexed Postgres projection.
- Keeps the operator toolchain consistent with ADR 0030: `agctl` for local/dev workflows, `trogon-gateway-ctl` for admin HTTP — neither pulls in a web stack.
- Defers a UI decision until there is real operator demand. We do not pay for a web console we cannot staff.

**Negative / Risks**

- Non-technical stakeholders cannot drive the CLI directly; they must ask an operator or consume the SIEM export (OCSF) through their existing tooling. Acceptable for v1 because the named user is "security engineer / on-call operator", not a product manager.
- Pretty-printing chain trees in a terminal is harder than in HTML. Mitigation: `--json` plus `jq` covers exploratory workflows; chain visualization graduates to a TUI (or web console) only if Block 4 follow-ups demand it.

**Out of scope**

- Web frontend, dashboards as a hosted service, multi-tenant SaaS console, SSO integration. Any of those requires a new ADR and a new workspace.
