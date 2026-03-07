//! Binary end-to-end tests for the `agent` binary.
//!
//! Spawns the compiled `agent` binary as an OS process and verifies its
//! startup and failure behaviour, mirroring the `binary_e2e.rs` suite that
//! already exists for `trogon-secret-proxy`.
//!
//! Run with:
//!   cargo test -p trogon-agent --test binary_e2e

use std::time::Duration;

use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt};
use tokio::process::Command;

// ── helpers ────────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

/// Create the GITHUB and LINEAR JetStream streams that the agent runner
/// requires before it will start processing events.
async fn create_required_streams(nats_port: u16) {
    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .expect("Failed to connect to NATS");
    let js = async_nats::jetstream::new(nats);

    for (name, subject) in [("GITHUB", "github.pull_request"), ("LINEAR", "linear.Issue.>"), ("CRON_TICKS", "cron.>")] {
        js.get_or_create_stream(async_nats::jetstream::stream::Config {
            name: name.to_string(),
            subjects: vec![subject.to_string()],
            ..Default::default()
        })
        .await
        .unwrap_or_else(|e| panic!("Failed to create {name} stream: {e}"));
    }
}

// ── tests ──────────────────────────────────────────────────────────────────────

/// The agent binary must exit with a non-zero status code when NATS is
/// unreachable (NATS_URL points to a port that has nothing listening).
///
/// Does NOT require Docker — the binary fails fast on the connect timeout.
#[tokio::test]
async fn binary_agent_exits_nonzero_when_nats_unreachable() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agent"))
        // Port 19999 is used here and is extremely unlikely to have a NATS
        // listener; if it does the test still passes once the agent fails to
        // find the required JetStream streams.
        .env("NATS_URL", "localhost:19999")
        .env("PROXY_URL", "http://localhost:8080")
        .env("ANTHROPIC_TOKEN", "tok_anthropic_prod_test01")
        .env("GITHUB_TOKEN", "tok_github_prod_test01")
        .env("LINEAR_TOKEN", "tok_linear_prod_test01")
        .env("RUST_LOG", "error")
        .spawn()
        .expect("Failed to spawn agent binary — run `cargo build -p trogon-agent` first");

    // trogon-nats may retry before giving up; allow 30 s to be safe.
    let status = tokio::time::timeout(Duration::from_secs(30), child.wait())
        .await
        .expect("Agent binary did not exit within 30 s when NATS is unreachable")
        .expect("Failed to wait for agent process");

    assert!(
        !status.success(),
        "Agent binary must exit non-zero when NATS is unreachable; got {:?}",
        status
    );
}

/// The agent binary starts successfully, connects to JetStream, binds both
/// consumers, and stays alive waiting for events (does not crash on startup).
#[tokio::test]
async fn binary_agent_starts_and_stays_alive_with_valid_config() {
    let (_nats_container, nats_port) = start_nats().await;
    create_required_streams(nats_port).await;

    let mut child = Command::new(env!("CARGO_BIN_EXE_agent"))
        .env("NATS_URL", format!("localhost:{nats_port}"))
        .env("PROXY_URL", "http://localhost:8080")
        .env("ANTHROPIC_TOKEN", "tok_anthropic_prod_test01")
        .env("GITHUB_TOKEN", "tok_github_prod_test01")
        .env("LINEAR_TOKEN", "tok_linear_prod_test01")
        .env("RUST_LOG", "error")
        .spawn()
        .expect("Failed to spawn agent binary — run `cargo build -p trogon-agent` first");

    // Give the binary time to connect to NATS and bind its consumers.
    tokio::time::sleep(Duration::from_millis(800)).await;

    // The process must still be running — it should block waiting for events.
    let exit = child
        .try_wait()
        .expect("Failed to poll agent process status");
    assert!(
        exit.is_none(),
        "Agent binary crashed during startup; exit status: {:?}",
        exit
    );

    child.kill().await.ok();
}

/// The agent binary exits non-zero when the required JetStream streams
/// (GITHUB, LINEAR) do not exist on the NATS server.
#[tokio::test]
async fn binary_agent_exits_nonzero_when_streams_missing() {
    let (_nats_container, nats_port) = start_nats().await;
    // Intentionally do NOT create the GITHUB / LINEAR streams.

    let mut child = Command::new(env!("CARGO_BIN_EXE_agent"))
        .env("NATS_URL", format!("localhost:{nats_port}"))
        .env("PROXY_URL", "http://localhost:8080")
        .env("ANTHROPIC_TOKEN", "tok_anthropic_prod_test01")
        .env("GITHUB_TOKEN", "tok_github_prod_test01")
        .env("LINEAR_TOKEN", "tok_linear_prod_test01")
        .env("RUST_LOG", "error")
        .spawn()
        .expect("Failed to spawn agent binary");

    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("Agent binary did not exit within 10 s when streams are missing")
        .expect("Failed to wait for agent process");

    assert!(
        !status.success(),
        "Agent binary must exit non-zero when JetStream streams are missing; got {:?}",
        status
    );
}
