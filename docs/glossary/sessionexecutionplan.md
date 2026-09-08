---
term: "SessionExecutionPlan"
section: "Agent execution model"
order: 6
---

# SessionExecutionPlan

The immutable record of the exact Agent revision, runtime, settings, platform
declarations, and resolved inputs admitted for one Session, frozen at
`SessionStarted`. It records the facts required by the admitted execution
capabilities. For platform-managed model access, these include model selections
derived and validated by a supported adapter and their resolved provider routes.
Live grants and plaintext credentials remain outside the plan. See draft
[ADR#0031](../adr/0031-agent-implementation-and-session-plan.md) and
[ADR#0062](../adr/0062-runtime-owned-settings-and-platform-declarations.md).
