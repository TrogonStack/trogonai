# Microsoft Agent Governance Toolkit — Research Set

Research package for porting the security and enterprise controls defined in [microsoft/agent-governance-toolkit](https://github.com/microsoft/agent-governance-toolkit) onto TrogonStack's NATS-native substrate.

Read in order:

1. [00-overview.md](./00-overview.md) — framing, strategic position, document map.
2. [01-component-map.md](./01-component-map.md) — AGT component ↔ TrogonStack crate (status grid).
3. [02-security-mechanisms.md](./02-security-mechanisms.md) — normative restatement of AGT's security primitives.
4. [03-enterprise-requirements.md](./03-enterprise-requirements.md) — enterprise / compliance controls implied by AGT.
5. [04-nats-native-translation.md](./04-nats-native-translation.md) — how each control re-projects onto NATS subjects, accounts, JetStream.
6. [05-gap-analysis.md](./05-gap-analysis.md) — 59 distinct gaps, severity-rated, owner-mapped.
7. [06-todo-backlog.md](./06-todo-backlog.md) — the actionable, dependency-ordered checklist.

Reference material:

- [standards/](./standards/) — crosswalks to OWASP Agentic, NIST AI RMF, EU AI Act, SOC 2, ISO 42001.
- [../../security/threat-model.md](../../security/threat-model.md) — STRIDE threat model per security-critical crate (closes gap **TM-01**).

The authoritative ship list is `06-todo-backlog.md`.
