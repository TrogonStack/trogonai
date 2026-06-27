//! UI fixture for the `telemetry_metric_name_literal` lint. Built as an example
//! so the fixture can depend on the real `opentelemetry` crate, whose `Meter`
//! the lint's type gate requires.

use opentelemetry::metrics::Meter;

const ACP_REQUESTS: &str = "acp.requests";

fn inline_name(meter: &Meter) {
    let _counter = meter.u64_counter("acp.requests").build();
}

fn constant_name(meter: &Meter) {
    let _counter = meter.u64_counter(ACP_REQUESTS).build();
}

fn unrelated_builder(fake: &FakeMeter) {
    let _ = fake.u64_counter("acp.requests");
}

struct FakeMeter;

impl FakeMeter {
    fn u64_counter(&self, _name: &str) -> u64 {
        0
    }
}

fn main() {
    let meter = opentelemetry::global::meter("fixture");
    inline_name(&meter);
    constant_name(&meter);
    unrelated_builder(&FakeMeter);
}
