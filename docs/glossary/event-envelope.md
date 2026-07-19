---
term: "Event envelope"
section: "Event sourcing and the decider"
order: 6
---

# Event envelope

The persisted wrapper around a domain payload, carrying operational metadata and
recorder-assigned time (`recorded_at`) that does not belong on the payload
itself. See [Event Metadata](../architecture/event-metadata.md).
