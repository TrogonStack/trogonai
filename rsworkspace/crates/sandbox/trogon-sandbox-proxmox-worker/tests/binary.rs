mod support;

use std::path::Path;
use std::process::Output;
use std::time::Duration;

use futures::StreamExt;
use serde_json::{Value, json};
use support::*;
use tokio::process::{Child, Command};
use trogon_sandbox_contract::{ProviderKind, WorkerId};
use trogon_sandbox_protocol::{nats_transfer, subjects};

const BINARY: &str = env!("CARGO_BIN_EXE_trogon-sandbox-proxmox-worker");
const UNUSED_NATS: &str = "nats://127.0.0.1:9";
/// Settings the binary reads that a test must not inherit from the machine running it.
const KNOBS: [&str; 16] = [
    "TROGON_SANDBOX_NATS_URL",
    "TROGON_SANDBOX_PROXMOX_URL",
    "TROGON_SANDBOX_PROXMOX_NODE",
    "TROGON_SANDBOX_PROXMOX_POOL",
    "TROGON_SANDBOX_PROXMOX_TOKEN_ID",
    "TROGON_SANDBOX_PROXMOX_TOKEN_SECRET",
    "TROGON_SANDBOX_PROXMOX_TEMPLATE",
    "TROGON_SANDBOX_PROXMOX_IMAGE",
    "TROGON_SANDBOX_PROXMOX_STATE_DIR",
    "TROGON_SANDBOX_PROXMOX_PROXY_BRIDGE",
    "TROGON_SANDBOX_PROXMOX_GUEST_ADDRESSES",
    "TROGON_SANDBOX_PROXMOX_GUEST_GATEWAY",
    "TROGON_SANDBOX_PROXMOX_PROXY_BIND",
    "TROGON_SANDBOX_PROXMOX_PROXY_ADDRESS",
    "TROGON_SANDBOX_WORKER_ID",
    "SANDBOX_WORKER_AUTHORITY",
];
const PROCESS_LIMIT: Duration = Duration::from_secs(90);

/// The binary with the required settings, the environment it would otherwise inherit cleared of its own knobs.
fn worker(nats_url: &str, endpoint: &str, state: &Path) -> Command {
    let mut command = Command::new(BINARY);
    for name in KNOBS {
        command.env_remove(name);
    }
    command
        .env("RUST_LOG", "off")
        .env("TROGON_SANDBOX_NATS_URL", nats_url)
        .env("TROGON_SANDBOX_PROXMOX_URL", endpoint)
        .env("TROGON_SANDBOX_PROXMOX_NODE", NODE)
        .env("TROGON_SANDBOX_PROXMOX_TOKEN_ID", TOKEN_ID)
        .env("TROGON_SANDBOX_PROXMOX_TOKEN_SECRET", uuid::Uuid::new_v4().to_string())
        .env("TROGON_SANDBOX_PROXMOX_TEMPLATE", "100")
        .env("TROGON_SANDBOX_PROXMOX_IMAGE", IMAGE)
        .env("TROGON_SANDBOX_PROXMOX_STATE_DIR", state)
        .kill_on_drop(true);
    command
}

/// Runs the binary until it exits on its own and returns what it printed.
async fn finish(mut command: Command) -> Boxed<Output> {
    let child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    Ok(tokio::time::timeout(PROCESS_LIMIT, child.wait_with_output()).await??)
}

async fn rejected(command: Command, message: &str) -> TestResult {
    let output = finish(command).await?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
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

/// Starts the binary and waits for its first heartbeat, which names the worker.
async fn serve(command: &mut Command, nats: &Nats) -> Boxed<(Child, WorkerId, Value)> {
    let client = nats.client().await?;
    let mut beats = client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::Proxmox))
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
    let beat: Value = serde_json::from_slice(&message.payload)?;
    let worker_id = serde_json::from_value(beat["worker_id"].clone())?;
    Ok((child, worker_id, beat))
}

async fn dispatch(nats: &Nats, worker_id: &WorkerId, lease: &str) -> Boxed<Value> {
    let client = nats.client().await?;
    let payload = json!({
        "lease_id": lease,
        "request": request_json(),
        "target": {"worker_id": worker_id.as_str(), "attempt_id": "attempt-1"},
    });
    let reply = nats_transfer::request(
        &client,
        &subjects::worker_dispatch(ProviderKind::Proxmox, worker_id),
        serde_json::to_vec(&payload)?.into(),
        None,
        Duration::from_secs(30),
    )
    .await?;
    Ok(serde_json::from_slice(&reply)?)
}

/// Stops a running worker with SIGINT and returns its exit status.
async fn stop(mut child: Child) -> Boxed<std::process::ExitStatus> {
    interrupt(&child)?;
    Ok(tokio::time::timeout(Duration::from_secs(60), child.wait()).await??)
}

#[tokio::test]
async fn a_full_run_serves_leases_and_destroys_its_vms_on_interrupt() -> TestResult {
    let nats = Nats::start().await;
    let fake = Fake::start().await?;
    let dir = tempfile::tempdir()?;
    let mut command = worker(&nats.url(), &fake.url(), dir.path());
    let (child, worker_id, beat) = serve(&mut command, &nats).await?;
    assert_eq!(beat["capacity"]["max_concurrent_leases"], 4);
    assert!(worker_id.as_str().starts_with("proxmox-"));
    let reply = dispatch(&nats, &worker_id, "lease-1").await?;
    assert!(reply.get("sandbox_id").is_some(), "not granted: {reply}");
    assert!(fake.clone_exists());
    let status = stop(child).await?;
    assert!(status.success(), "{status}");
    assert!(!fake.clone_exists());
    Ok(())
}

#[tokio::test]
async fn the_lease_limit_comes_from_the_command_line() -> TestResult {
    let nats = Nats::start().await;
    let fake = Fake::start().await?;
    let dir = tempfile::tempdir()?;
    let mut command = worker(&nats.url(), &fake.url(), dir.path());
    command.args(["--max-leases", "1"]);
    let (child, _, beat) = serve(&mut command, &nats).await?;
    assert_eq!(beat["capacity"]["max_concurrent_leases"], 1);
    let status = stop(child).await?;
    assert!(status.success(), "{status}");
    Ok(())
}

#[tokio::test]
async fn a_restart_keeps_the_worker_identity() -> TestResult {
    let nats = Nats::start().await;
    let fake = Fake::start().await?;
    let dir = tempfile::tempdir()?;
    let mut command = worker(&nats.url(), &fake.url(), dir.path());
    let (child, first, _) = serve(&mut command, &nats).await?;
    assert!(stop(child).await?.success());
    let mut command = worker(&nats.url(), &fake.url(), dir.path());
    let (child, second, _) = serve(&mut command, &nats).await?;
    assert_eq!(first, second);
    assert!(stop(child).await?.success());
    Ok(())
}

#[tokio::test]
async fn a_configured_worker_id_that_differs_from_the_durable_identity_is_refused() -> TestResult {
    let fake = Fake::start().await?;
    let dir = tempfile::tempdir()?;
    let mut command = worker(UNUSED_NATS, &fake.url(), dir.path());
    command.env("TROGON_SANDBOX_WORKER_ID", "someone-else");
    rejected(command, "configured worker id differs from its durable identity").await
}

#[tokio::test]
async fn a_second_worker_cannot_share_a_state_directory() -> TestResult {
    let nats = Nats::start().await;
    let fake = Fake::start().await?;
    let dir = tempfile::tempdir()?;
    let mut first = worker(&nats.url(), &fake.url(), dir.path());
    let (child, _, _) = serve(&mut first, &nats).await?;
    rejected(
        worker(&nats.url(), &fake.url(), dir.path()),
        "another worker owns this state directory",
    )
    .await?;
    assert!(stop(child).await?.success());
    Ok(())
}

#[tokio::test]
async fn a_partial_proxy_network_is_refused() -> TestResult {
    let fake = Fake::start().await?;
    let dir = tempfile::tempdir()?;
    let mut command = worker(UNUSED_NATS, &fake.url(), dir.path());
    command.env("TROGON_SANDBOX_PROXMOX_PROXY_BRIDGE", "vmbr9");
    rejected(command, "configure all proxy network settings together").await
}

#[tokio::test]
async fn a_remote_endpoint_without_tls_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let command = worker(UNUSED_NATS, "http://192.0.2.1:8006", dir.path());
    rejected(command, "remote API endpoints require HTTPS").await
}

#[tokio::test]
async fn missing_required_settings_are_refused() -> TestResult {
    let fake = Fake::start().await?;
    let dir = tempfile::tempdir()?;
    let mut command = worker(UNUSED_NATS, &fake.url(), dir.path());
    command.env_remove("TROGON_SANDBOX_PROXMOX_TEMPLATE");
    rejected(command, "--template").await
}

#[tokio::test]
async fn an_unreachable_nats_server_stops_the_worker() -> TestResult {
    let fake = Fake::start().await?;
    let dir = tempfile::tempdir()?;
    rejected(worker(UNUSED_NATS, &fake.url(), dir.path()), "connecting to NATS").await
}

fn report_lines(output: &Output) -> Boxed<Vec<Value>> {
    let mut lines = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        lines.push(serde_json::from_str(line)?);
    }
    Ok(lines)
}

#[tokio::test]
async fn the_spike_flag_checks_the_host_and_exits_without_nats() -> TestResult {
    let fake = Fake::start_with(spike_host()).await?;
    let dir = tempfile::tempdir()?;
    let mut command = worker(UNUSED_NATS, &fake.url(), dir.path());
    command.arg("--spike");
    let output = finish(command).await?;
    assert!(
        output.status.success(),
        "spike failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines = report_lines(&output)?;
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["phase"], "granted");
    assert_eq!(lines[1]["phase"], "released");
    assert_eq!(lines[1]["destroy_verified"], true);
    assert!(!fake.clone_exists());
    Ok(())
}

#[tokio::test]
async fn a_spike_against_a_guest_that_fails_its_checks_exits_with_an_error() -> TestResult {
    let fake = Fake::start().await?;
    let dir = tempfile::tempdir()?;
    let mut command = worker(UNUSED_NATS, &fake.url(), dir.path());
    command.arg("--spike");
    rejected(command, "did not match").await?;
    assert!(!fake.clone_exists());
    Ok(())
}

#[tokio::test]
#[ignore = "requires a live Proxmox VE host and template; set the TROGON_SANDBOX_PROXMOX_* variables the worker reads"]
async fn the_spike_passes_against_a_live_host() -> TestResult {
    let mut command = Command::new(BINARY);
    command.arg("--spike").kill_on_drop(true);
    let output = finish(command).await?;
    assert!(
        output.status.success(),
        "spike failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines = report_lines(&output)?;
    assert!(lines.iter().any(|line| line["phase"] == "released"));
    Ok(())
}
