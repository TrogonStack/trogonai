---
term: "Retention watermark"
section: "Event sourcing and the decider"
order: 16
---

# Retention watermark

The snapshot-derived point below which stream events can be truncated because no
replay will need them again. Defined by
[ADR#0029](../adr/0029-decider-retention-and-truncation-watermark.md) and
computed by `trogon-decider-nats`; the purge that would act on it is not
implemented, and nothing in the store issues one.
