Prefer domain-specific value objects over primitives (e.g. `AcpPrefix` not `String`). Each type's factory must guarantee correctness at construction—invalid instances should be unrepresentable. Validate per-type, not per-aggregate: avoid validating unrelated fields together in a single constructor.

Untrusted input must use distinct `*Input` / `*Wire` / `*Request` types. Convert those boundary types into domain types exactly once. After conversion, domain values must be valid forever. Do not add `validate_foo(...)` helpers over a supposed domain struct; if a struct still needs a validator to be safe, it is not a domain type yet. Persist only validated domain types to storage, events, and runtime state. Never persist wire/input types.

Every value object lives in its own file named after the type (e.g. `acp_prefix.rs`, `ext_method_name.rs`, `session_id.rs`). Never inline a value object into a config, aggregate, or service file. File layout: `src/{type_snake_case}.rs`; export in `lib.rs` as `pub use {module}::{Type, TypeError}`.

Errors must be typed—use structs or enums, never `String` or `format!()`. Every error type must implement `Display` and `std::error::Error`. Never discard error context by converting a typed error into a string; wrap the source error as a field or variant instead.

You must use the `test-support` feature to share test helpers between crates.
Prefer one trait per operation over a single trait with multiple operations.

Production implementations of infrastructure traits must be zero-cost passthroughs to the underlying SDK. No error wrapping (use the SDK's error types directly via associated `type Error`), no return type conversion (add associated types like `type Info` to match the SDK's return), no `map_err`, no `map(|_| ())`. The impl body should be `self.sdk_field.method(args).await` — nothing else. Conversion logic (e.g. `Cursor::new`, `read_to_end`) belongs in the caller, not the passthrough.

For NATS infrastructure and testing, use the `trogon-nats` crate which provides:
- `NatsClient` trait for testability
- Connection management with auto-reconnect
- Request/publish utilities with retry policies
- Mock NATS clients (via `test-support` feature)

## Module conventions

Place observability concerns (metrics, tracing spans, logging helpers) under a `telemetry` module within each crate. Example: `acp-nats/src/telemetry/metrics.rs`. This keeps observability code separated from domain logic and provides a consistent location across crates.
