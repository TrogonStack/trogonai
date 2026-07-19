---
term: "ExecutionAttempt"
section: "Agent execution model"
order: 7
---

# ExecutionAttempt

One launch or restart of a session, an append-only sequence of facts about that
attempt (started, ready, ended). A restart creates a new attempt under the same
immutable plan. See [ADR#0031](../adr/0031-agent-implementation-and-session-plan.md).
