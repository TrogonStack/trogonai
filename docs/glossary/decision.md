---
term: "Decision"
section: "Event sourcing and the decider"
order: 5
---

# Decision

The result of `decide`: either a flat batch of `Events`, or an `Act` that
observes the state produced by an earlier step before deciding the next one. See
[Decider Platform](../architecture/decider.md).
