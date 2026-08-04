---
term: "Checkpoint"
section: "Event sourcing and the decider"
order: 11
---

# Checkpoint

The last physical stream sequence a projection or consumer has processed, so it
can resume without reprocessing. This read-side position is not an aggregate
[Snapshot](./snapshot) and is not the `trogonai.session.sessions.v1alpha1.Checkpoint`
protobuf. [ADR#0031](../adr/0031-agent-implementation-and-session-plan.md) and
[ADR#0035](../adr/0035-session-store-decider-aggregate.md) call that opaque
platform harness state a harness recovery checkpoint to keep the failure modes
distinct.
See [Decider Platform](../architecture/decider.md#read-side-primitives-projector-and-processor).
