//! End-to-end smoke test for the `run` shim.
//!
//! Drives the public `a2a_gateway::run` entrypoint with valid defaults so
//! the production thin shim is exercised through the same call path the
//! binary uses.

#![allow(clippy::expect_used)]

use a2a_gateway::{Args, run};

#[tokio::test(flavor = "current_thread")]
async fn run_completes_with_valid_defaults() {
    let args = Args {
        nats_url: "localhost:4222".to_string(),
        prefix: "a2a".to_string(),
        queue_group: None,
    };
    run(args).await.expect("bootstrap-only run succeeds");
}
