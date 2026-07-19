---
term: "Snapshot"
section: "Event sourcing and the decider"
order: 10
---

# Snapshot

A persisted fold of state at a stream position, letting a command replay only the
events after it instead of the whole stream. Snapshots are advisory: an untrusted
one falls back to a full replay. See [Decider Platform](../architecture/decider.md).
