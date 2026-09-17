//! UI fixture proving `telemetry_key_value_literal` is suppressed in test files.
//! The example is named `*_tests.rs`, so the same inline `KeyValue::new` string
//! key that fires in `telemetry_key_value_literal.rs` produces no diagnostic
//! here. Absent a `.stderr`, the harness asserts zero diagnostics.

use opentelemetry::KeyValue;

fn inline_key() -> KeyValue {
    KeyValue::new("messaging.system", "nats")
}

fn main() {
    let _ = inline_key();
}
