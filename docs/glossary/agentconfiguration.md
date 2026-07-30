---
term: "AgentConfiguration"
section: "Agent execution model"
order: 1
---

# AgentConfiguration

The versioned record holding an agent's runtime selection and the runtime-owned
settings that runtime defines and validates, including its model selection.
Digest-committed, so any behavior-significant change produces a new revision.
Whether model selection instead belongs to the platform as a typed field is
unresolved; see the consequence recorded in
[ADR#0025](../adr/0025-agent-definition-data-ownership.md) and
[ADR#0031](../adr/0031-agent-implementation-and-session-plan.md).
