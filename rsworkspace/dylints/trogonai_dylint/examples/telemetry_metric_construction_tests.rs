//! UI fixture proving `telemetry_metric_construction` is suppressed in test
//! files. The example is named `*_tests.rs`, so opening an instrument builder
//! inline, which is denied in `telemetry_metric_construction.rs`, produces no
//! diagnostic here. Absent a `.stderr`, the harness asserts zero diagnostics.

use opentelemetry::metrics::{Counter, Meter};

const ACP_REQUESTS: &str = "acp.requests";

fn inline_build(meter: &Meter) -> Counter<u64> {
    meter
        .u64_counter(ACP_REQUESTS)
        .with_description("Total number of ACP requests")
        .build()
}

fn main() {
    let meter = opentelemetry::global::meter("fixture");
    let _ = inline_build(&meter);
}
