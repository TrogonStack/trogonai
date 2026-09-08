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
  normal operation, as observed by `gateway.credential.revocation.latency`
  evaluated as a rolling 5-minute p99.
- Alerting: page when that p99 exceeds 10 seconds sustained over 5 minutes,
  or when the histogram stops reporting while credential traffic continues.
  An evaluation window holding fewer than 20 revocation samples is skipped
  rather than paged on: a tail percentile over a handful of events is noise,
  and the missing-data alert already covers silence. The sample floor is a
  working value like the latency numbers.
- The target and alert are all-replica, evaluated per replica. Invalidation
  is fan-out, each replica invalidating its own cache through its own
  consumer, because queue-group delivery would leave the other replicas
  serving a revoked credential
  ([ADR#0023](./0023-secret-management-and-key-custody-direction.md)). The
  histogram therefore carries its emitting instance's resource identity
  ([ADR#0008](./0008-opentelemetry-observability.md)), evaluation groups by
  instance, and one replica sustaining a p99 above the alert threshold pages
  even when the fleet-wide aggregate looks healthy. The sample floor and the
  missing-data alert apply per replica as well.
- The cache TTL plus jitter (at most 330 seconds) is the hard upper bound on
  staleness when the event path fails entirely; the alert on the event path
  exists precisely so the backstop is never the operative mechanism.
- The target is scoped to credentials this platform resolves. It does not
  extend to standing that has been federated to an external identity
  provider, which never consults this platform's revocation state; that
  boundary is bounded by assertion TTL plus the remote provider's own cache
  and is decided in
  [ADR#0053](./0053-external-oidc-federation-surface.md). An offboarding that
  must hold on both planes is not complete when this histogram says it is.
- The numbers are working values. They are revisited once production stream
  metrics exist, and any change lands as an amendment to this ADR.

## Consequences

- Alert definitions have a concrete threshold to encode.
- Event-driven invalidation through the checkpointed projection refresh is
  sized against the 5-second target.
- "What revocation latency is required" is settled as a ratified working value
  rather than left open.

## References

- [ADR#0008: OpenTelemetry Observability](./0008-opentelemetry-observability.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0046: Project-Anchored Resource Hierarchy for the Credential Platform](./0046-project-anchored-resource-hierarchy.md)
