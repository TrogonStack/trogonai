You must use the `test-support` feature to share test helpers between crates.
Prefer one trait per operation over a single trait with multiple operations.

For NATS infrastructure and testing, use the `trogon-nats` crate which provides:
- `NatsClient` trait for testability
- Connection management with auto-reconnect
- Request/publish utilities with retry policies
- Mock NATS clients (via `test-support` feature)
