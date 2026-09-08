#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! `a2a-nats-stdio` — bridges JSON-RPC requests over stdin/stdout to an
//! `A2aClient` running against a real NATS server. See the library docs for
//! the protocol shape.

#[tokio::main]
#[cfg_attr(
    dylint_lib = "trogon_lints",
    allow(
        debug_remnants,
        reason = "the bridge exits before a subscriber is installed, so stderr is the only channel left to report on"
    )
)]
#[cfg_attr(coverage_nightly, coverage(off))]
async fn main() {
    if let Err(e) = a2a_nats_stdio::run().await {
        eprintln!("a2a-nats-stdio error: {e}");
        std::process::exit(1);
    }
}
