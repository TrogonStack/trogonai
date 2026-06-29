//! UI fixture for the `telemetry_metric_construction` lint. Built as an example
//! so the fixture can depend on the real `opentelemetry` crate, whose `Meter`
//! the lint's type gate requires. The example crate is not `trogon-semconv`, so
//! opening an instrument builder here is denied.

use opentelemetry::metrics::{Counter, Meter};

const ACP_REQUESTS: &str = "acp.requests";

fn inline_build(meter: &Meter) -> Counter<u64> {
    meter
        .u64_counter(ACP_REQUESTS)
        .with_description("Total number of ACP requests")
        .build()
}

fn unrelated_builder(fake: &FakeMeter) {
    let _ = fake.u64_counter("x");
}

struct FakeMeter;

impl FakeMeter {
    fn u64_counter(&self, _name: &str) -> u64 {
        0
    }
}

fn main() {
    let meter = opentelemetry::global::meter("fixture");
    let _ = inline_build(&meter);
    unrelated_builder(&FakeMeter);
}
