---
term: "Fork"
section: "Agent execution model"
order: 18
---

# Fork

A new [session](./session) that continues from a shared prefix of a source
session's history. A fork mints its own identity and retains
`source_session_id` for lineage and prefix resolution; copied or inherited
facts never masquerade as belonging to the source. The shared prefix is
composed by reference through a context projection, not physically copied.
Distinct from a [child session](./child-session), which is new work delegated
by a parent rather than a branch of the same conversation. See
[ADR#0035](../adr/0035-session-store-decider-aggregate.md).
