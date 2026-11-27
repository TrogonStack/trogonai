mod support;

use std::process::Output;
use std::time::Duration;

use futures::StreamExt;
use serde_json::Value;
use support::*;
use tokio::process::{Child, Command};
use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_protocol::subjects;

const BINARY: &str = env!("CARGO_BIN_EXE_trogon-sandbox-modal-worker");
const FAKE_PYTHON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fake_python.sh");
const UNUSED_NATS: &str = "nats://127.0.0.1:9";
/// Settings the binary reads that a test must not inherit from the machine running it.
const KNOBS: [&str; 14] = [
    "TROGON_SANDBOX_NATS_URL",
    "TROGON_SANDBOX_DISPATCH_FENCE_FILE",
    "TROGON_SANDBOX_MODAL_APP_NAME",
    "TROGON_SANDBOX_MODAL_ENVIRONMENT",
    "TROGON_SANDBOX_MODAL_CPU_CORES",
    "TROGON_SANDBOX_MAX_CONCURRENT_LEASES",
    "TROGON_SANDBOX_MODAL_REGIONS",
    "TROGON_SANDBOX_MODAL_MAX_VCPUS",
    "TROGON_SANDBOX_MODAL_PYTHON",
    "TROGON_SANDBOX_MODAL_VOLUME_CREDENTIAL_ID",
    "TROGON_SANDBOX_MODAL_VOLUME_REGION",
    "TROGON_SANDBOX_MODAL_VOLUME_ENDPOINT",
    "SANDBOX_WORKER_AUTHORITY",
    "SANDBOX_METRICS_ADDR",
];
const PROCESS_LIMIT: Duration = Duration::from_secs(90);

fn worker(nats_url: &str) -> Command {
    let mut command = Command::new(BINARY);
    for name in KNOBS {
        command.env_remove(name);
    }
    command
        .env("RUST_LOG", "off")
        .env("TROGON_SANDBOX_NATS_URL", nats_url)
        .env("TROGON_SANDBOX_MODAL_PYTHON", FAKE_PYTHON)
        .kill_on_drop(true);
    command
}

async fn finish(mut command: Command) -> Boxed<Output> {
    let child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    Ok(tokio::time::timeout(PROCESS_LIMIT, child.wait_with_output()).await??)
}

fn interrupt(child: &Child) -> TestResult {
    let pid = child.id().ok_or("the worker already exited")?;
    let status = std::process::Command::new("kill")
        .args(["-INT", &pid.to_string()])
        .status()?;
    assert!(status.success());
    Ok(())
}

/// Starts the binary and waits for its first heartbeat.
async fn serve(command: &mut Command, nats: &Nats) -> Boxed<(Child, Value)> {
    let client = nats.client().await?;
    let mut beats = client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::Modal))
        .await?;
    client.flush().await?;
    let child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let message = tokio::time::timeout(Duration::from_secs(60), beats.next())
        .await?
        .ok_or("no heartbeat")?;
    Ok((child, serde_json::from_slice(&message.payload)?))
}

async fn stop(mut child: Child) -> Boxed<std::process::ExitStatus> {
    interrupt(&child)?;
    Ok(tokio::time::timeout(Duration::from_secs(60), child.wait()).await??)
}

#[tokio::test]
async fn a_default_run_serves_until_interrupted() -> TestResult {
    let nats = Nats::start().await;
    let mut command = worker(&nats.url());
    let (child, beat) = serve(&mut command, &nats).await?;
    assert_eq!(beat["provider"], "modal");
    assert_eq!(beat["capacity"]["max_concurrent_leases"], 4);
    let status = stop(child).await?;
    assert!(status.success(), "{status}");
    Ok(())
}

#[tokio::test]
async fn a_configured_run_reports_its_settings() -> TestResult {
    let nats = Nats::start().await;
    let fence = tempfile::tempdir()?;
    let mut command = worker(&nats.url());
    command
        .env("TROGON_SANDBOX_MODAL_ENVIRONMENT", "staging")
        .env("TROGON_SANDBOX_MODAL_VOLUME_ENDPOINT", "http://127.0.0.1:9000")
        .env("TROGON_SANDBOX_DISPATCH_FENCE_FILE", fence.path().join("fence"))
        .args([
            "--max-concurrent-leases",
            "2",
            "--regions",
            "us-east,eu-west",
            "--cpu-cores",
            "0.5",
        ]);
    let (child, beat) = serve(&mut command, &nats).await?;
    assert_eq!(beat["capacity"]["max_concurrent_leases"], 2);
    let status = stop(child).await?;
    assert!(status.success(), "{status}");
    Ok(())
}

#[tokio::test]
async fn empty_optional_settings_count_as_unset() -> TestResult {
    let nats = Nats::start().await;
    let mut command = worker(&nats.url());
    command
        .env("TROGON_SANDBOX_MODAL_ENVIRONMENT", "")
        .env("TROGON_SANDBOX_MODAL_VOLUME_ENDPOINT", "");
    let (child, _) = serve(&mut command, &nats).await?;
    let status = stop(child).await?;
    assert!(status.success(), "{status}");
    Ok(())
}

#[tokio::test]
async fn invalid_settings_are_refused_before_anything_starts() -> TestResult {
    let cases: [&[&str]; 5] = [
        &["--app-name", "two words"],
        &["--cpu-cores", "0"],
        &["--volume-region", "bad region!"],
        &["--volume-endpoint", "not a url"],
        &["--max-vcpus", "many"],
    ];
    for arguments in cases {
        let mut command = worker(UNUSED_NATS);
        command.args(arguments);
        let output = finish(command).await?;
        assert!(!output.status.success(), "{arguments:?} succeeded");
    }
    Ok(())
}

#[tokio::test]
async fn an_unreachable_nats_server_fails_the_start() -> TestResult {
    let output = finish(worker(UNUSED_NATS)).await?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("connecting to NATS"));
    Ok(())
}

#[tokio::test]
async fn an_invalid_authority_mode_is_refused() -> TestResult {
    let nats = Nats::start().await;
    let mut command = worker(&nats.url());
    command.env("SANDBOX_WORKER_AUTHORITY", "sometimes");
    let output = finish(command).await?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("sometimes"));
    Ok(())
}
