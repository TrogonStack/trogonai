#![cfg(unix)]

mod support;

use std::process::{ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde_json::Value;
use support::{TestResult, key};
use tokio::process::{Child, Command};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_protocol::subjects;
use trogon_std::env::{ReadEnv, SystemEnv};

const BINARY: &str = env!("CARGO_BIN_EXE_trogon-sandbox-daytona-worker");
const UNUSED_ENDPOINT: &str = "http://127.0.0.1:9";
const UNUSED_NATS: &str = "nats://127.0.0.1:9";
/// Settings the binary reads that a test must not inherit from the machine running it.
const KNOBS: [&str; 16] = [
    "SANDBOX_NATS_URL",
    "DAYTONA_API_KEY",
    "DAYTONA_ORGANIZATION_ID",
    "DAYTONA_BASE_URL",
    "SANDBOX_DAYTONA_SNAPSHOT",
    "SANDBOX_DAYTONA_HTTP_TIMEOUT_SECS",
    "SANDBOX_DAYTONA_START_TIMEOUT_SECS",
    "SANDBOX_MAX_CONCURRENT_LEASES",
    "SANDBOX_DISPATCH_FENCE_FILE",
    "SANDBOX_DAYTONA_DISPATCH_LEDGER_FILE",
    "SANDBOX_DISPATCH_VERIFY",
    "SANDBOX_DISPATCH_PUBLIC_KEYS",
    "SANDBOX_WORKER_AUTHORITY",
    "SANDBOX_OTLP_ENDPOINT",
    "SANDBOX_METRICS_ADDR",
    "RUST_LOG",
];
const PROCESS_LIMIT: Duration = Duration::from_secs(90);

/// The binary with the required settings, cleared of the knobs it would otherwise inherit. Coverage instrumentation
/// reaches the child through the inherited `LLVM_PROFILE_FILE`.
fn worker(nats_url: &str) -> Command {
    let mut command = Command::new(BINARY);
    for name in KNOBS {
        command.env_remove(name);
    }
    command
        .env("DAYTONA_API_KEY", key())
        .env("RUST_LOG", "off")
        .env("SANDBOX_NATS_URL", nats_url)
        .env("DAYTONA_BASE_URL", UNUSED_ENDPOINT)
        .args(["--metrics-addr", "127.0.0.1:0"])
        .kill_on_drop(true);
    if let Some(profile) = SystemEnv.var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    command
}

/// Runs the binary until it exits on its own and returns what it printed.
async fn finish(mut command: Command) -> Result<(Output, Duration), Box<dyn std::error::Error + Send + Sync>> {
    let started = Instant::now();
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let output = tokio::time::timeout(PROCESS_LIMIT, child.wait_with_output()).await??;
    Ok((output, started.elapsed()))
}

async fn rejected(command: Command, message: &str) -> TestResult {
    let (output, _) = finish(command).await?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "unexpected success");
    assert!(stderr.contains(message), "stderr lacks {message:?}: {stderr}");
    Ok(())
}

fn interrupt(child: &Child) -> TestResult {
    let pid = child.id().ok_or("the worker already exited")?;
    let status = std::process::Command::new("kill")
        .args(["-INT", &pid.to_string()])
        .status()?;
    assert!(status.success());
    Ok(())
}

/// Interrupts the worker and waits a bounded time for a clean exit, killing it if it does not leave.
async fn stop(mut child: Child) -> Result<ExitStatus, Box<dyn std::error::Error + Send + Sync>> {
    interrupt(&child)?;
    if let Ok(status) = tokio::time::timeout(Duration::from_secs(60), child.wait()).await {
        return Ok(status?);
    }
    child.kill().await?;
    Err("the worker ignored the interrupt".into())
}

#[tokio::test]
async fn a_started_worker_heartbeats_and_exits_cleanly_on_interrupt() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let client = nats.client().await;
    let mut beats = client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::Daytona))
        .await?;
    client.flush().await?;
    let mut command = worker(&format!("nats://{}", nats.address()));
    let child = command
        .args(["--max-concurrent-leases", "3"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let message = tokio::time::timeout(Duration::from_secs(60), beats.next())
        .await?
        .ok_or("no heartbeat")?;
    let beat: Value = serde_json::from_slice(&message.payload)?;
    assert_eq!(beat["capacity"]["max_concurrent_leases"], 3);
    let status = stop(child).await?;
    assert!(status.success(), "{status}");
    Ok(())
}

#[tokio::test]
async fn an_unreachable_nats_server_fails_quickly() -> TestResult {
    let (output, elapsed) = finish(worker(UNUSED_NATS)).await?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("connecting to NATS"), "{stderr}");
    assert!(elapsed < Duration::from_secs(30), "{elapsed:?}");
    Ok(())
}

#[tokio::test]
async fn an_unusable_nats_url_exits_nonzero() -> TestResult {
    rejected(worker("not a url"), "connecting to NATS").await
}

#[tokio::test]
async fn invalid_settings_are_refused_before_anything_starts() -> TestResult {
    let cases: [&[&str]; 6] = [
        &["--http-timeout-secs", "0"],
        &["--start-timeout-secs", "0"],
        &["--api-url", "http://example.com"],
        &["--api-url", "not a url"],
        &["--api-key", ""],
        &["--organization-id", ""],
    ];
    for arguments in cases {
        let mut command = worker(UNUSED_NATS);
        command.args(arguments);
        let (output, _) = finish(command).await?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{arguments:?} succeeded");
        assert!(stderr.contains("error"), "{arguments:?}: {stderr}");
    }
    Ok(())
}

#[tokio::test]
async fn a_missing_api_key_is_refused() -> TestResult {
    let mut command = worker(UNUSED_NATS);
    command.env_remove("DAYTONA_API_KEY");
    rejected(command, "--api-key").await
}

#[tokio::test]
async fn an_invalid_authority_mode_is_refused() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let mut command = worker(&format!("nats://{}", nats.address()));
    command.env("SANDBOX_WORKER_AUTHORITY", "sometimes");
    rejected(command, "sometimes").await
}
