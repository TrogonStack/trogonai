#![cfg(unix)]

mod support;

use std::process::{ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde_json::Value;
use support::TestResult;
use tokio::process::{Child, Command};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_protocol::subjects;

const BINARY: &str = env!("CARGO_BIN_EXE_trogon-sandbox-vercel-worker");
const UNUSED_ENDPOINT: &str = "http://127.0.0.1:9";
const UNUSED_NATS: &str = "nats://127.0.0.1:9";
/// Settings the binary reads that a test must not inherit from the machine running it.
const KNOBS: [&str; 18] = [
    "SANDBOX_NATS_URL",
    "SANDBOX_VERCEL_API_URL",
    "SANDBOX_VERCEL_RUNTIME",
    "SANDBOX_VERCEL_PLAN",
    "SANDBOX_VERCEL_MAX_VCPUS",
    "SANDBOX_VERCEL_HTTP_TIMEOUT_SECS",
    "SANDBOX_VERCEL_REGIONS",
    "SANDBOX_MAX_CONCURRENT_LEASES",
    "SANDBOX_DISPATCH_FENCE_FILE",
    "SANDBOX_VERCEL_DISPATCH_LEDGER_FILE",
    "SANDBOX_DISPATCH_VERIFY",
    "SANDBOX_DISPATCH_PUBLIC_KEYS",
    "SANDBOX_WORKER_AUTHORITY",
    "SANDBOX_OTLP_ENDPOINT",
    "SANDBOX_METRICS_ADDR",
    "VERCEL_TOKEN",
    "VERCEL_TEAM_ID",
    "VERCEL_PROJECT_ID",
];
const PROCESS_LIMIT: Duration = Duration::from_secs(90);

/// The binary with the required settings, the environment it would otherwise inherit cleared of its own knobs.
fn worker(nats_url: &str) -> Command {
    let mut command = Command::new(BINARY);
    for name in KNOBS {
        command.env_remove(name);
    }
    command
        .env("VERCEL_TOKEN", "test-token")
        .env("VERCEL_TEAM_ID", "team_1")
        .env("VERCEL_PROJECT_ID", "prj_1")
        .env("RUST_LOG", "off")
        .env("SANDBOX_NATS_URL", nats_url)
        .env("SANDBOX_VERCEL_API_URL", UNUSED_ENDPOINT)
        .args(["--metrics-addr", "127.0.0.1:0"])
        .kill_on_drop(true);
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

async fn stop(mut child: Child) -> Result<ExitStatus, Box<dyn std::error::Error + Send + Sync>> {
    interrupt(&child)?;
    Ok(tokio::time::timeout(Duration::from_secs(60), child.wait()).await??)
}

#[tokio::test]
async fn a_started_worker_heartbeats_and_exits_cleanly_on_interrupt() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let client = nats.client().await;
    let mut beats = client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::Vercel))
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
async fn an_unknown_runtime_exits_nonzero_before_connecting() -> TestResult {
    let mut command = worker(UNUSED_NATS);
    command.env("SANDBOX_VERCEL_RUNTIME", "node18");
    rejected(command, "SANDBOX_VERCEL_RUNTIME").await
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
    let cases: [&[&str]; 8] = [
        &["--plan", "team"],
        &["--max-vcpus", "0"],
        &["--http-timeout-secs", "0"],
        &["--api-url", "http://example.com"],
        &["--api-url", "not a url"],
        &["--token", ""],
        &["--token", "line\nbreak"],
        &["--team-id", ""],
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
async fn missing_credentials_are_refused() -> TestResult {
    let mut command = worker(UNUSED_NATS);
    command.env_remove("VERCEL_PROJECT_ID");
    rejected(command, "--project-id").await
}

#[tokio::test]
async fn an_invalid_authority_mode_is_refused() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let mut command = worker(&format!("nats://{}", nats.address()));
    command.env("SANDBOX_WORKER_AUTHORITY", "sometimes");
    rejected(command, "sometimes").await
}
