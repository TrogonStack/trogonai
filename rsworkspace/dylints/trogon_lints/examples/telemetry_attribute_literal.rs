//! UI fixture for the `telemetry_attribute_literal` lint. Built as an example
//! so the fixture can depend on the real `tracing` crate, which the lint's
//! type gate (`tracing::Span`) requires.

use tracing::Span;

const SESSION_ID: &str = "session_id";

fn inline_key(span: &Span) {
    span.record("session_id", "value");
}

fn constant_key(span: &Span) {
    span.record(SESSION_ID, "value");
}

fn unrelated_record(map: &Recorder) {
    map.record("session_id", "value");
}

struct Recorder;

impl Recorder {
    fn record(&self, _field: &str, _value: &str) {}
}

fn main() {
    let span = Span::current();
    inline_key(&span);
    constant_key(&span);
    unrelated_record(&Recorder);
}
