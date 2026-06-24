---
number: "0010"
slug: testcontainers-for-infrastructure-tests
status: accepted
date: 2026-06-09
---

# ADR 0010: Unit Tests First, Testcontainers Only When Necessary

## Context

The repository has service and transport code that depends on infrastructure
with behavior that is difficult to model faithfully with mocks alone. NATS,
JetStream, object stores, databases, queues, and webhook-facing services can
fail because of protocol behavior, network lifecycle, readiness, persistence,
ordering, permissions, or server-specific configuration.

Unit tests with mocks, contract tests, and in-memory fakes are the preferred
default for domain logic, package-local behavior, and most regression coverage.
They should not be replaced with containerized tests when the dependency
behavior is not essential to the assertion.

When a test does need the real dependency, ad hoc `docker run` instructions,
shared developer services, and manually provisioned local state make test results
depend on the machine running them. The repository needs one repeatable default
for infrastructure-backed integration tests.

## Decision

Prefer unit tests with mocks, contract tests, and in-memory fakes. Use
Testcontainers only when the test needs a real infrastructure dependency and the
behavior cannot be validated with a lighter test boundary.

Before adding a Testcontainers-backed test, identify the infrastructure behavior
that cannot be represented with mocks or fakes. If that behavior is not central
to the assertion, keep the test in the faster unit or contract test layer.

Testcontainers is appropriate only when the test needs to validate:

- Protocol behavior from a real server.
- Connection lifecycle, reconnects, timeouts, or readiness handling.
- Persistence, stream, queue, lease, object-store, or transaction behavior.
- Authentication, authorization, or server-side configuration.
- Cross-process integration between first-party code and infrastructure.
- Regression coverage for a failure that only appears against the real service.

Do not use Testcontainers for ordinary service logic, happy paths that mocks can
cover, serialization, parsing, request routing, configuration merging, value
objects, or any regression where a mock or fake can prove the same behavior.

## Test Design Rules

Container-backed tests should be deterministic and self-contained:

- Start dependencies through Testcontainers-owned fixtures or helpers.
- Use dynamic ports and container-provided connection details.
- Wait for explicit readiness before the test exercises the system.
- Create isolated subjects, streams, buckets, databases, schemas, or tenants per
  test when state can leak across assertions.
- Clean up test-owned state through the fixture lifecycle.
- Keep infrastructure setup close to the package that owns the integration.
- Share reusable container fixtures only after at least two packages need the
  same behavior.
- Keep image names, versions, and required environment variables visible in the
  test fixture or package test support module.

Tests must not depend on a developer's manually running `docker run` process, a
shared local service, or unstated host configuration. Developer-facing examples
and exploratory commands may still prefer local services such as
`*.ubi.orb.local`, but automated test success must come from test-owned
infrastructure.

## Exceptions

Do not use Testcontainers when a lighter test boundary gives the same confidence.
Valid exceptions include:

- Pure domain logic, value objects, serialization, parsing, validation, routing
  rules, and configuration behavior.
- Unit tests with mocks or protocol contract tests where a local fake can prove
  the same request and response shape.
- Tests that would become flaky because the required dependency cannot be made
  deterministic in a container.
- External managed services whose important behavior is not represented by an
  available local container image.
- Deployment, performance, or operations tests that intentionally target a
  provisioned environment instead of the local test harness.

When an exception skips Testcontainers for infrastructure behavior, document the
reason near the test or package boundary that owns the decision.

## Consequences

- Unit tests with mocks, contract tests, and in-memory fakes remain the expected
  default for fast feedback and focused regressions.
- Container-backed integration tests are narrow, intentional, and reserved for
  real infrastructure behavior.
- Reviewers should challenge Testcontainers usage when mocks or fakes can prove
  the same behavior.
- Reviewers can ask for a Testcontainers-backed regression only when a change
  depends on server behavior that mocks cannot prove.
- Manual Docker setup should move out of automated test instructions when a
  Testcontainers fixture can own the same dependency.
- CI runners need a working container runtime for infrastructure-backed test
  suites.

## References

- [ADR 0003: AI Protocol Transport Taxonomy](./0003-ai-protocol-transport-taxonomy.md)
- [ADR 0004: Protocol and Transport Layering](./0004-protocol-and-transport-layering.md)
- [ADR 0009: Protocol Buffers Wire Contracts](./0009-protocol-buffers-wire-contracts.md)
- [Testcontainers](https://testcontainers.com/)
