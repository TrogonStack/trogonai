//! UI fixture for the `telemetry_key_value_literal` lint. Built as an example
//! so the fixture can depend on the real `opentelemetry` crate, whose
//! `KeyValue::new` the lint's type gate requires.

use opentelemetry::KeyValue;

static MESSAGING_SYSTEM: &str = "messaging.system";

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

trait Keyed {
    fn new(key: &str) -> Self;
}

impl Keyed for Wrapper {
    fn new(_key: &str) -> Self {
        Wrapper
    }
}

/// A call that resolves to a trait's own associated fn named `new` has a trait,
/// not an `impl`, as its parent. The lint has to recognise that before asking
/// for a self type.
fn trait_associated_new<T: Keyed>() -> T {
    T::new("messaging.system")
}

fn main() {
    let _ = inline_key();
    let _ = constant_key();
    let _ = unrelated_new();
    let _: Wrapper = trait_associated_new();
}
