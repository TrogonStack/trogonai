---
term: "Admission control"
section: "Event sourcing and the decider"
order: 15
---

# Admission control

Bounding which commands are allowed to execute under load, applying backpressure
rather than accepting unbounded work. Defined by
[ADR#0028](../adr/0028-decider-admission-control-and-backpressure.md) and
implemented as an opt-in limiter on both decider execution paths.
