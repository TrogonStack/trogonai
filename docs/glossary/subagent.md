---
term: "Subagent"
section: "Agent execution model"
order: 15
---

# Subagent

Informal, product-facing name for a [child session](./child-session): a session
spawned by another session's run to do a delegated subtask. The name says "agent",
but the entity is a session (one run of an agent), so the precise model term is
child session, not subagent. See
[ADR#0035](../adr/0035-session-store-decider-aggregate.md).
