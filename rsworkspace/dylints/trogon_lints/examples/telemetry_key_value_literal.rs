//! UI fixture for the `telemetry_key_value_literal` lint. Built as an example
//! so the fixture can depend on the real `opentelemetry` crate, whose
//! `KeyValue::new` the lint's type gate requires.

use opentelemetry::KeyValue;

const MESSAGING_SYSTEM: &str = "messaging.system";

fn inline_key() -> KeyValue {
    KeyValue::new("messaging.system", "nats")
}

fn constant_key() -> KeyValue {
    KeyValue::new(MESSAGING_SYSTEM, "nats")
}

fn unrelated_new() -> Wrapper {
    Wrapper::new("messaging.system")
}

struct Wrapper;

impl Wrapper {
    fn new(_key: &str) -> Self {
        Wrapper
    }
}

fn main() {
    let _ = inline_key();
    let _ = constant_key();
    let _ = unrelated_new();
}
