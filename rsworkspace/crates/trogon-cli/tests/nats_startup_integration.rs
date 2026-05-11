//! Integration tests for connect_or_start_nats.
//!
//! Requires Docker (testcontainers spins up a real NATS container).
//!
//! Run with:
//!   cargo test -p trogon-cli --test nats_startup_integration

use std::time::Duration;

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

/// When NATS is already running, connects immediately and returns no child process.
#[tokio::test]
async fn already_running_connects_without_spawning_child() {
    let (_container, port) = start_nats().await;
    let url = format!("nats://127.0.0.1:{port}");

    let (client, child) = connect_or_start_nats(&url, Duration::from_secs(3))
        .await
        .expect("should connect to running NATS");

    assert!(child.is_none(), "no child should be spawned when NATS is already up");
    client.flush().await.expect("client should be usable");
}

/// When nothing is listening and nats-server is not in PATH, returns a helpful error.
#[tokio::test]
async fn nats_server_not_in_path_returns_error() {
    let url = "nats://127.0.0.1:14222"; // nothing listening here

    let result = connect_or_start_nats_with_empty_path(url, Duration::from_secs(1)).await;

    let err = result.expect_err("should fail when nats-server is not in PATH");
    let msg = err.to_string();
    assert!(msg.contains("nats-server is not in PATH"), "unexpected error: {msg}");
    assert!(msg.contains("https://docs.nats.io"), "should include install URL: {msg}");
}

/// When a fake nats-server binary is in PATH but never accepts connections,
/// the function times out and returns a clear error.
#[tokio::test]
async fn fake_nats_server_that_never_listens_returns_timeout_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let fake_bin = dir.path().join("nats-server");

    // A script that exists but never binds a port.
    std::fs::write(&fake_bin, "#!/bin/sh\nsleep 60\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&fake_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let url = "nats://127.0.0.1:14223"; // nothing listening here
    let timeout = Duration::from_millis(600); // short so the test is fast

    let result = connect_or_start_nats_with_path(url, timeout, dir.path()).await;

    let err = result.expect_err("should time out");
    let msg = err.to_string();
    assert!(
        msg.contains("not accepting connections"),
        "unexpected error: {msg}"
    );
}

// ── Helpers that override PATH ────────────────────────────────────────────────

async fn connect_or_start_nats_with_empty_path(
    url: &str,
    timeout: Duration,
) -> anyhow::Result<(async_nats::Client, Option<std::process::Child>)> {
    connect_or_start_nats_with_path(url, timeout, std::path::Path::new("")).await
}

async fn connect_or_start_nats_with_path(
    url: &str,
    timeout: Duration,
    path_dir: &std::path::Path,
) -> anyhow::Result<(async_nats::Client, Option<std::process::Child>)> {
    // Temporarily replace PATH so Command::new("nats-server") resolves (or not)
    // as we control. Restore it afterward even if we panic.
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    std::env::set_var("PATH", path_dir);
    let result = connect_or_start_nats(url, timeout).await;
    std::env::set_var("PATH", old_path);
    result
}
