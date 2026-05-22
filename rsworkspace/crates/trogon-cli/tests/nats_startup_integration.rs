//! Integration tests for connect_or_start_nats.
//!
//! Requires Docker (testcontainers spins up a real NATS container).
//!
//! Run with:
//!   cargo test -p trogon-cli --test nats_startup_integration

use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, runners::AsyncRunner};
use trogon_cli::connect_or_start_nats;

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

/// When NATS is already running, connects and returns a usable client.
#[tokio::test]
async fn connects_to_running_nats() {
    let (_container, port) = start_nats().await;
    let url = format!("nats://127.0.0.1:{port}");

    let client = connect_or_start_nats(&url)
        .await
        .expect("should connect to running NATS");

    client.flush().await.expect("client should be usable");
}

/// When nothing is listening, returns an error with actionable guidance.
#[tokio::test]
async fn nothing_listening_returns_clear_error() {
    let url = "nats://127.0.0.1:14222"; // nothing listening here

    let err = connect_or_start_nats(url)
        .await
        .expect_err("should fail when NATS is not running");

    let msg = err.to_string();
    assert!(msg.contains("nats-server --jetstream"), "should suggest jetstream flag: {msg}");
    assert!(msg.contains("TROGON_NATS_URL"), "should mention env var: {msg}");
}
