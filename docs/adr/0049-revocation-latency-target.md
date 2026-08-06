---
number: "0049"
slug: revocation-latency-target
status: accepted
date: 2026-08-05
---

# ADR#0049: Revocation Propagation Latency Target

## Context

Revocation latency has to be measured against a stated target for the
platform's revocation guarantee to mean anything operationally. The
measurement side now exists: the gateway records the
`gateway.credential.revocation.latency` histogram (seconds) from a revocation
event's broker-recorded timestamp to the moment the runtime projection
invalidates the cached credential. Three mechanisms bound staleness today:
event-driven invalidation through the checkpointed projection refresh,
fail-closed resolution for revoked or disabled credentials once the
projection reflects them, and the cache TTL of 300 seconds with up to 30
seconds of deterministic per-key jitter as the backstop when an invalidation
event is missed.

A target number is a promise about operational behavior, so it is recorded
here as the platform's working service objective rather than left implicit
in dashboards.

## Decision

- Target: p99 revocation-to-invalidation latency at or under 5 seconds under
  normal operation, as observed by `gateway.credential.revocation.latency`.
- Alerting: page when the p99 exceeds 10 seconds sustained over 5 minutes,
  or when the histogram stops reporting while credential traffic continues.
- The cache TTL plus jitter (at most 330 seconds) is the hard upper bound on
  staleness when the event path fails entirely; the alert on the event path
  exists precisely so the backstop is never the operative mechanism.
- The numbers are working values. They are revisited once production stream
  metrics exist, and any change lands as an amendment to this ADR.

## Consequences

- Alert definitions have a concrete threshold to encode.
- Outbox-driven invalidation is sized against the 5 second target.
- "What revocation latency is required" is settled as a ratified working value
  rather than left open.

## References

- [ADR#0008: OpenTelemetry Observability](./0008-opentelemetry-observability.md)
- [ADR#0046: Project-Anchored Resource Hierarchy for the Credential Platform](./0046-project-anchored-resource-hierarchy.md)
