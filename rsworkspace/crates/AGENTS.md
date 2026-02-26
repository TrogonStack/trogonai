Prefer domain-specific value objects over primitives (e.g. `AcpPrefix` not `String`). Each type's factory must guarantee correctness at construction—invalid instances should be unrepresentable. Validate per-type, not per-aggregate: avoid validating unrelated fields together in a single constructor.

When validating strings for NATS subjects or domain constraints, introduce a value object in its own file (e.g. `ext_method_name.rs`, `session_id.rs`) rather than a standalone `validate_*` function or adding it to `config.rs`. The value object's constructor performs validation; invalid instances are unrepresentable.

You must use the `test-support` feature to share test helpers between crates.
Prefer one trait per operation over a single trait with multiple operations.

For NATS infrastructure and testing, use the `trogon-nats` crate which provides:
- `NatsClient` trait for testability
- Connection management with auto-reconnect
- Request/publish utilities with retry policies
- Mock NATS clients (via `test-support` feature)

## Module conventions

Place observability concerns (metrics, tracing spans, logging helpers) under a `telemetry` module within each crate. Example: `acp-nats/src/telemetry/metrics.rs`. This keeps observability code separated from domain logic and provides a consistent location across crates.
