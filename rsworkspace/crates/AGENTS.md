Prefer domain-specific value objects over primitives (e.g. `AcpPrefix` not `String`). Each type's factory must guarantee correctness at construction—invalid instances should be unrepresentable. Validate per-type, not per-aggregate: avoid validating unrelated fields together in a single constructor.

Every value object lives in its own file named after the type (e.g. `acp_prefix.rs`, `ext_method_name.rs`, `session_id.rs`). Never inline a value object into a config, aggregate, or service file. File layout: `src/{type_snake_case}.rs`; export in `lib.rs` as `pub use {module}::{Type, TypeError}`.

You must use the `test-support` feature to share test helpers between crates.
Prefer one trait per operation over a single trait with multiple operations.

For NATS infrastructure and testing, use the `trogon-nats` crate which provides:
- `NatsClient` trait for testability
- Connection management with auto-reconnect
- Request/publish utilities with retry policies
- Mock NATS clients (via `test-support` feature)

## Module conventions

Place observability concerns (metrics, tracing spans, logging helpers) under a `telemetry` module within each crate. Example: `acp-nats/src/telemetry/metrics.rs`. This keeps observability code separated from domain logic and provides a consistent location across crates.
