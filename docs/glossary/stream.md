---
term: "Stream"
section: "Event sourcing and the decider"
order: 8
---

# Stream

The ordered sequence of events for one entity. A logical stream is one entity's
events; many logical streams share one physical JetStream stream, one subject
each. See [Decider Platform](../architecture/decider.md#one-physical-stream-subject-per-logical-stream)
and [ADR#0024](../adr/0024-agent-platform-stream-topology.md).
