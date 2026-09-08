---
term: "Child session"
section: "Agent execution model"
order: 14
---

# Child session

A [session](./session) created by another session's run to carry out a delegated
unit of work, related to its parent by lineage (a `parent_session_id`), a
[delegation](./delegation) operation, and a [cascade policy](./cascade-policy). It
is a full, independent session -- its own agent revision, execution plan,
transcript, and terminal outcome -- not a lesser or embedded one. A parent may have
many children; each child has exactly one parent, so the relationship is a directed
tree, never a peer mesh. Informally called a "[subagent](./subagent)". Distinct from
a [fork](./fork) (which shares a prefix of the same conversation) and an
[external delegated agent](./external-delegated-agent) (which is not a session at
all). See [ADR#0035](../adr/0035-session-store-decider-aggregate.md) and
[ADR#0031](../adr/0031-agent-implementation-and-session-plan.md).
