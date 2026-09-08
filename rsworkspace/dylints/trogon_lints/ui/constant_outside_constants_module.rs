// edition:2024
#![allow(dead_code)]

// Module-level constant in a non-`constants` file: this is the violation.
const MAX_INSPECTED_BODY: usize = 1024 * 1024;

// A `static` is a different construct and is out of scope: must NOT be linted.
static GREETING: &str = "hi";

// Explicitly allowed at the site: must NOT be linted.
#[allow(constant_outside_constants_module)]
const ALLOWED: usize = 1;

struct Widget;

impl Widget {
    // Associated const (an impl item, not a free item): must NOT be linted.
    const WHEELS: u8 = 4;
}

fn compute() -> u8 {
    // Function-local constant is a local implementation detail: must NOT be
    // linted.
    const LOCAL: u8 = 7;
    LOCAL
}

// A constant inside an inline `tests` module is test scaffolding, not crate
// configuration: must NOT be linted. (`#[allow(inline_module_block)]` keeps the
// unrelated inline-module lint quiet in this fixture.)
#[allow(inline_module_block)]
mod tests {
    const FIXTURE_TIMEOUT: u8 = 30;
}

// Constants inside the not-for-prod test-support module family (`test_support`,
// `mocks`, `*_harness`, …) are scaffolding: must NOT be linted.
#[allow(inline_module_block)]
mod test_support {
    const MOCK_LATENCY_MS: u8 = 5;
}
#[allow(inline_module_block)]
mod mocks {
    const MOCK_BATCH_ID: &str = "batch-1";
}
#[allow(inline_module_block)]
mod nats_harness {
    const HARNESS_TENANT: &str = "tenant-test";
}

fn main() {
    let _ = compute();
}
