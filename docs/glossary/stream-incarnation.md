---
term: "Stream incarnation"
section: "Event sourcing and the decider"
order: 17
---

# Stream incarnation

A label on the physical JetStream stream that holds an aggregate's events,
advancing only when the platform rebuilds that stream. It is carried as a token
in the subject, so two incarnations occupy disjoint subject spaces and a writer
holding a position from a retired one cannot reach the live one. Retiring an
incarnation seals it, permanently rejecting writes. See
[ADR#0057](../adr/0057-session-stream-incarnation-fencing.md).
