---
term: "SessionExecutionPlan"
section: "Agent execution model"
order: 6
---

# SessionExecutionPlan

The immutable record of the exact revision, implementation, models, provider
routes, and dependencies admitted for one session, frozen at `SessionStarted`.
Recording admitted models presumes the platform can read the agent's model
selection, which runtime-owned settings do not currently grant. See
[ADR#0031](../adr/0031-agent-implementation-and-session-plan.md) and the
consequence recorded in
[ADR#0025](../adr/0025-agent-definition-data-ownership.md).
