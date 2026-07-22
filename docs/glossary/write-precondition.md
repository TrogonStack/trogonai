---
term: "Write precondition"
section: "Event sourcing and the decider"
order: 9
---

# Write precondition

The optional concurrency guard applied when decided events are appended: `Any`,
`StreamExists`, or `NoStream` (first writer wins). See
[Decider Platform](../architecture/decider.md).
