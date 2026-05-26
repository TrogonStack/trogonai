# Microsoft Agent Governance Toolkit — Research Overview

> Source repository: <https://github.com/microsoft/agent-governance-toolkit>
> Snapshot location: `/tmp/agt-research/agent-governance-toolkit/`
> Scope: identify every governance / security / enterprise control AGT defines that TrogonStack does not yet implement, and translate the design to a NATS-native shape.

## Why AGT Matters For TrogonStack

The Microsoft Agent Governance Toolkit (AGT) is the most complete public specification of an **agent control plane**. It bundles, under a single coherent set of normative specs, the controls enterprises ask for when adopting MCP / A2A / tool-using agents:

| Concern                       | AGT Spec                                      |
| ----------------------------- | --------------------------------------------- |
| Policy decisions              | `AGENT-OS-POLICY-ENGINE-1.0`                  |
| Workload / agent identity     | `AGENTMESH-IDENTITY-TRUST-1.0`                |
| MCP runtime mediation         | `MCP-SECURITY-GATEWAY-1.0`                    |
| Execution privilege gating    | `AGENT-HYPERVISOR-EXECUTION-CONTROL-1.0`      |
| Tamper-evident audit / BOM    | `AUDIT-COMPLIANCE-1.0`                        |
| SRE / SLO / chaos             | `AGENT-SRE-GOVERNANCE-1.0`                    |
| E2E confidentiality           | `AGENTMESH-WIRE-1.0`                          |
| Verifiable receipts proposal  | `proposals/verifiable-compliance-receipts.md` |
| STRIDE threat model           | `docs/security/threat-model.md`               |
| Tenant isolation              | `docs/security/tenant-isolation.md`           |

AGT is implemented as **Python application-layer interception** sitting inside the agent process. Its core architectural assumption is "deterministic middleware in front of the LLM call".

TrogonStack's architecture is **bus-native**: every agent, gateway and policy decision is a NATS subject. This means we cannot copy AGT 1:1 — instead we must re-host its *security model* on top of subjects, accounts, JetStream, queue-groups, services and account-resolver mints.

## Strategic Position

AGT is **product-shaped middleware**. TrogonStack is **infrastructure-shaped substrate**. The right relationship is therefore:

1. **Adopt AGT's normative vocabulary** (DIDs, trust tiers, execution rings, decision BOM, drift categories, conflict-resolution operators) wherever it is well-defined. This buys us interop and compliance language for free.
2. **Reject AGT's transport assumptions** (HTTP+Python middleware, per-process audit chain, single-tenant Merkle ledger). Replace with NATS-native equivalents (JetStream audit streams per-tenant, account-isolated gateways, subject-scoped queue services).
3. **Mark every "agent process owns the security guarantee" assumption as a non-goal.** Our security must hold even when the agent code is hostile — because in TrogonStack, the agent is on the other side of an account boundary from the gateway.

## Document Set

| File                                 | Purpose                                                                  |
| ------------------------------------ | ------------------------------------------------------------------------ |
| `00-overview.md`                     | This file. Framing.                                                      |
| `01-component-map.md`                | AGT component → TrogonStack equivalent (where it exists or doesn't).     |
| `02-security-mechanisms.md`          | Full catalog of AGT's security primitives, normatively summarised.       |
| `03-enterprise-requirements.md`      | Enterprise + compliance controls (SOC 2, ISO 42001, EU AI Act, NIST).    |
| `04-nats-native-translation.md`      | How each control re-projects onto NATS accounts/subjects/JetStream.      |
| `05-gap-analysis.md`                 | Per-domain gap matrix with severity, owner crate, and risk.              |
| `06-todo-backlog.md`                 | **The actionable checklist** to walk through one by one.                 |

## Ground Rules For This Research

- We do not copy code from AGT. We extract requirements only. AGT is MIT-licensed but TrogonStack is licensed independently; copying Python middleware into Rust crates buys us nothing.
- We treat every AGT "MUST" as an enterprise-customer-facing requirement worth shipping unless we have a concrete reason to disagree.
- We treat every AGT "SHOULD" as a backlog candidate.
- We treat AGT's *process-local* assumptions (e.g. "the middleware can mutate the LLM request") as anti-requirements — TrogonStack cannot rely on trusting the agent process.
- Every gap turns into a checklist item in `06-todo-backlog.md`.
