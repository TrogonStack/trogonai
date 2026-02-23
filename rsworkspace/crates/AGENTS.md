You must use the `test-support` feature to share test helpers between crates.
Prefer one trait per operation over a single trait with multiple operations.

For NATS infrastructure and testing, use the `trogon-nats` crate which provides:
- `NatsClient` trait for testability
- Connection management with auto-reconnect
- Request/publish utilities with retry policies
- Mock NATS clients (via `test-support` feature)

For distributed CRON scheduling, use the `trogon-cron` crate which provides:
- `Scheduler` — runs the tick loop; only one instance fires per cluster (leader election via NATS KV TTL)
- `CronClient` — register/remove/enable/disable jobs stored in NATS KV (`cron_configs` bucket)
- Two schedule types: `interval_sec` (fixed interval) or `cron` (6-field expression: sec min hour dom month dow)
- Two action types: `publish` (NATS subject) or `spawn` (process with optional timeout and concurrency guard)
- Tick payloads include `job_id`, `fired_at`, `execution_id`, and optional `payload` forwarded from the job config
- OTel trace context is injected in tick message headers automatically
- Integration tests in `tests/integration.rs` (run with `NATS_TEST_URL=... cargo test -- --include-ignored`)
- Binary: `trogon-cron serve` / `trogon-cron job list|add|get|remove|enable|disable`
- `test-support` feature exposes `MockTickPublisher` (records published ticks) and `MockLeaderLock` (controllable via `deny_acquire()`/`deny_renew()`) for unit testing without a real NATS server
