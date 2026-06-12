---
number: "0008"
slug: opentelemetry-observability
status: accepted
date: 2026-06-08
---

# ADR 0008: OpenTelemetry Observability

## Context

The repository needs one default observability model for production-operated
services, apps, CLIs, SDKs, and workspace automation. Without one default, logs,
metrics, and traces can drift into vendor-specific libraries, incompatible
attribute names, and uncorrelated runtime views.

ADR 0001 already treats OpenTelemetry service identity as part of a service
boundary. ADR 0002 names shared observability setup as the responsibility of a
dedicated reusable package such as `trogon-telemetry`.

OpenTelemetry is the standard observability vocabulary that covers traces,
metrics, logs, resource attributes, semantic conventions, context propagation,
and vendor-neutral export.

## Decision

Prefer OpenTelemetry for logs, metrics, and traces in first-party code unless
there is a strictly necessary reason to use something else.

New observability work should model telemetry through OpenTelemetry concepts
first:

- Use OpenTelemetry resource attributes for runtime identity, including
  `service.name` and, when useful, `service.namespace` and
  `service.instance.id`.
- Use OpenTelemetry spans and context propagation for tracing.
- Use OpenTelemetry metric instruments and semantic conventions for metrics.
- Use structured log records that can carry OpenTelemetry trace and span
  correlation.
- Prefer OTLP-compatible export paths, normally through an OpenTelemetry
  Collector or a deployment-provided collector endpoint.
- Keep package-owned instruments, spans, and log attributes near the package
  that emits them.
- Put shared setup, exporters, resource construction, and reusable observability
  value objects in a named reusable package such as `trogon-telemetry`.

Inside a package, reserve `telemetry/` for observability code: logging setup,
tracing spans and attributes, metrics instruments, metric recorders, and small
helpers that make the package's behavior observable. Package-specific metrics
stay with the package that emits them.

Do not put general runtime integration code in `telemetry/`. Service-owned
connections to NATS, databases, queues, external APIs, storage, or other
infrastructure belong in a module named for the integration or adapter it owns,
such as `nats/`, `postgres/`, `jetstream/`, `connectrpc/`, or `adapters/` when
one service composes several private integrations. Extract the integration to a
reusable package only when another package should depend on that boundary.

Do not introduce a vendor SDK, logging framework, metrics client, tracing
library, or runtime-specific telemetry abstraction as the primary observability
boundary when OpenTelemetry can satisfy the requirement.

## Exceptions

Using something other than OpenTelemetry requires a concrete constraint, not a
preference. Valid exceptions include:

- A platform, deployment system, or third-party integration only exposes a
  non-OpenTelemetry interface.
- A vendor, compliance, or operations requirement mandates a specific non-OTLP
  format or API.
- The relevant OpenTelemetry language SDK or signal support is not mature enough
  for the required production behavior.
- A measured hot-path performance constraint cannot be met with the available
  OpenTelemetry implementation.
- Security or privacy requirements need a local redaction, filtering, or audit
  path before telemetry can enter an OpenTelemetry pipeline.

When an exception is used, bridge or normalize the data back to OpenTelemetry as
close to the boundary as practical. The exception must be documented at the
package, service, or deployment boundary that owns it.

## Consequences

- Observability code should preserve correlation across logs, metrics, and
  traces.
- Code review should reject new vendor-first telemetry abstractions unless they
  satisfy the exception rules.
- Package and service names should be chosen with stable OpenTelemetry resource
  identity in mind.
- A backend choice such as Prometheus, Grafana, Datadog, Honeycomb, or another
  observability platform should not leak into first-party instrumentation unless
  a documented exception requires it.
- Existing non-OpenTelemetry instrumentation can be migrated when touched for
  related work.

## References

- [ADR 0001: Workspace Runtime Taxonomy](/adr/0001-workspace-runtime-taxonomy)
- [ADR 0002: Rust Crate Boundaries](/adr/0002-rust-crate-boundaries)
- [OpenTelemetry Signals](https://opentelemetry.io/docs/concepts/signals/)
- [OpenTelemetry Logs](https://opentelemetry.io/docs/concepts/signals/logs/)
- [OpenTelemetry Metrics](https://opentelemetry.io/docs/concepts/signals/metrics/)
- [OpenTelemetry Specification](https://opentelemetry.io/docs/specs/otel/)
