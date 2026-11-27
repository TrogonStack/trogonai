#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = trogon_sandbox_egress_proxy::fuzz_support::chunked_body(data);
});
