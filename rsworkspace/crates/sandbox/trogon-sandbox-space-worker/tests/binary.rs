mod nats_common;
mod support;

use std::process::{Command, Stdio};
use std::time::Duration;

use futures::StreamExt;
use support::{MANAGEMENT_TOKEN, Space, TestResult};

const BINARY: &str = env!("CARGO_BIN_EXE_trogon-sandbox-space-worker");

fn worker(space: &str, nats: &str, state: &std::path::Path) -> Command {
    let mut command = Command::new(BINARY);
    command
        .args([
            "--endpoint",
            space,
            "--data-plane-url",
            space,
            "--nats-url",
            nats,
            "--state-dir",
        ])
        .arg(state)
        .env("SPACE_API_TOKEN", MANAGEMENT_TOKEN)
        .env_remove("SANDBOX_WORKER_AUTHORITY")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command
}

#[tokio::test(flavor = "multi_thread")]
async fn exits_with_an_error_for_an_unknown_authority_mode() -> TestResult {
    let dir = tempfile::tempdir()?;
    let output = worker("http://127.0.0.1:1", "nats://127.0.0.1:1", dir.path())
        .env("SANDBOX_WORKER_AUTHORITY", "sometimes")
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    assert!(!output.stderr.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_until_interrupted() -> TestResult {
    let space = Space::start().await;
    let (server, client) = nats_common::start_nats().await?;
    let mut beats = client.subscribe("sandbox.worker.heartbeat.>").await?;
    client.flush().await?;
    let dir = tempfile::tempdir()?;
    let mut child = worker(&space.uri(), &server.url(), dir.path()).spawn()?;
    tokio::time::timeout(Duration::from_secs(30), beats.next())
        .await?
        .ok_or("no heartbeat")?;
    let signal = Command::new("kill").args(["-INT", &child.id().to_string()]).status()?;
    assert!(signal.success());
    let status = tokio::task::spawn_blocking(move || child.wait()).await??;
    assert!(status.success());
    Ok(())
}
