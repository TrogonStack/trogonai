---
term: "Fork"
section: "Agent execution model"
order: 18
---

# Fork

A new [session](./session) that continues from a shared prefix of a source
session's history, minting its own identity and never referencing the source id;
the shared prefix is composed at replay time, not physically copied. Distinct from
a [child session](./child-session), which is new work delegated by a parent rather
than a branch of the same conversation. See
[ADR#0035](../adr/0035-session-store-decider-aggregate.md).
