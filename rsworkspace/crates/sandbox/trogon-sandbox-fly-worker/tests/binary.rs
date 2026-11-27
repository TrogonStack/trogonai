mod support;

use std::path::Path;
use std::process::Output;
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde_json::{Value, json};
use support::*;
use tokio::process::{Child, Command};
use trogon_sandbox_contract::{ProviderKind, WorkerId};
use trogon_sandbox_protocol::{nats_transfer, subjects};

const BINARY: &str = env!("CARGO_BIN_EXE_trogon-sandbox-fly-worker");
const UNUSED_ENDPOINT: &str = "http://127.0.0.1:9";
const UNUSED_NATS: &str = "nats://127.0.0.1:9";
/// Settings the binary reads that a test must not inherit from the machine running it.
const KNOBS: [&str; 18] = [
    "TROGON_SANDBOX_NATS_URL",
    "TROGON_SANDBOX_FLY_API_URL",
    "TROGON_SANDBOX_FLY_APP",
    "FLY_API_TOKEN",
    "TROGON_SANDBOX_FLY_IMAGE",
    "TROGON_SANDBOX_FLY_REGION",
    "TROGON_SANDBOX_FLY_STATE_DIR",
    "TROGON_SANDBOX_FLY_VOLUME",
    "TROGON_SANDBOX_FLY_VOLUME_TENANT",
    "TROGON_SANDBOX_FLY_VOLUME_DRIVE",
    "TROGON_SANDBOX_FLY_WORKSPACE_VOLUME_GB",
    "TROGON_SANDBOX_FLY_SNAPSHOT_POLL_INTERVAL_SECS",
    "TROGON_SANDBOX_FLY_SNAPSHOT_TIMEOUT_SECS",
    "TROGON_SANDBOX_EGRESS_PROXY",
    "TROGON_SANDBOX_WORKER_ID",
    "TROGON_SANDBOX_FLY_PUBLIC_HOST",
    "TROGON_SANDBOX_FLY_PREVIEW_NETWORK",
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
        .env("FLY_API_TOKEN", TOKEN)
        .env("RUST_LOG", "off")
        .env("TROGON_SANDBOX_NATS_URL", nats_url)
        .env("TROGON_SANDBOX_FLY_API_URL", endpoint)
        .env("TROGON_SANDBOX_FLY_APP", APP)
        .env("TROGON_SANDBOX_FLY_IMAGE", IMAGE)
        .env("TROGON_SANDBOX_FLY_REGION", "iad")
        .env("TROGON_SANDBOX_FLY_STATE_DIR", state)
        .kill_on_drop(true);
    command
}

/// Runs the binary until it exits on its own and returns what it printed.
async fn finish(mut command: Command) -> Boxed<(Output, Duration)> {
    let started = Instant::now();
    let child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let output = tokio::time::timeout(PROCESS_LIMIT, child.wait_with_output()).await??;
    Ok((output, started.elapsed()))
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

async fn rejected(command: Command, message: &str) -> TestResult {
    let (output, _) = finish(command).await?;
    let stderr = stderr_of(&output);
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
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::Fly))
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
        &subjects::worker_dispatch(ProviderKind::Fly, worker_id),
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
async fn a_full_run_serves_leases_and_destroys_its_machines_on_interrupt() -> TestResult {
    let nats = Nats::start().await;
    let fly = Fly::start().await;
    let dir = tempfile::tempdir()?;
    let mut command = worker(&nats.url(), &fly.url(), dir.path());
    let (child, worker_id, beat) = serve(&mut command, &nats).await?;
    assert_eq!(beat["capacity"]["max_concurrent_leases"], 4);
    let reply = dispatch(&nats, &worker_id, "lease-1").await?;
    assert!(reply.get("sandbox_id").is_some(), "not granted: {reply}");
    assert_eq!(fly.machines().len(), 1);
    let status = stop(child).await?;
    assert!(status.success(), "{status}");
    assert!(fly.machines().is_empty());
    Ok(())
}

#[tokio::test]
async fn an_operator_volume_run_serves_a_lease() -> TestResult {
    let nats = Nats::start().await;
    let fly = Fly::start().await;
    let dir = tempfile::tempdir()?;
    let mut command = worker(&nats.url(), &fly.url(), dir.path());
    command
        .args([
            "--volume",
            "vol_test",
            "--volume-tenant",
            "tenant-a",
            "--volume-drive",
            "data",
        ])
        .args(["--max-leases", "1"]);
    let (child, _, beat) = serve(&mut command, &nats).await?;
    assert_eq!(beat["capacity"]["max_concurrent_leases"], 1);
    assert!(
        beat["native_volume_bindings"]
            .as_array()
            .is_some_and(|bindings| bindings.len() == 1)
    );
    let status = stop(child).await?;
    assert!(status.success(), "{status}");
    Ok(())
}

#[tokio::test]
async fn a_workspace_volume_run_serves_a_lease() -> TestResult {
    let nats = Nats::start().await;
    let fly = Fly::start().await;
    let dir = tempfile::tempdir()?;
    let mut command = worker(&nats.url(), &fly.url(), dir.path());
    command.args(["--workspace-volume-size-gb", "1"]);
    let (child, worker_id, _) = serve(&mut command, &nats).await?;
    let reply = dispatch(&nats, &worker_id, "lease-1").await?;
    assert!(reply.get("sandbox_id").is_some(), "not granted: {reply}");
    assert_eq!(fly.volumes().len(), 1);
    let status = stop(child).await?;
    assert!(status.success(), "{status}");
    assert!(fly.machines().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_warm_pool_run_creates_a_machine_before_serving() -> TestResult {
    let nats = Nats::start().await;
    let fly = Fly::start().await;
    let dir = tempfile::tempdir()?;
    let mut command = worker(&nats.url(), &fly.url(), dir.path());
    command.args(["--warm-pool-size", "1", "--max-leases", "2"]);
    let (child, _, _) = serve(&mut command, &nats).await?;
    assert_eq!(fly.machines().len(), 1);
    let status = stop(child).await?;
    assert!(status.success(), "{status}");
    assert!(fly.machines().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_volume_without_a_tenant_and_drive_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut command = worker(UNUSED_NATS, UNUSED_ENDPOINT, dir.path());
    command.args(["--volume", "vol_test"]);
    rejected(
        command,
        "volume, volume tenant and volume drive must be configured together",
    )
    .await
}

#[tokio::test]
async fn invalid_settings_are_refused_before_anything_starts() -> TestResult {
    let dir = tempfile::tempdir()?;
    let volume = [
        "--volume",
        "vol_test",
        "--volume-tenant",
        "tenant-a",
        "--volume-drive",
        "data",
    ];
    let cases: Vec<(Vec<&str>, &str)> = vec![
        (vec!["--endpoint", "not a url"], "Error"),
        (vec!["--endpoint", "http://example.com"], "Error"),
        (vec!["--app", "bad app!"], "Error"),
        (vec!["--region", "bad region!"], "Error"),
        (vec!["--max-leases", "0"], "Error"),
        (vec!["--max-leases", "1", "--warm-pool-size", "5"], "Error"),
        (vec!["--boot-timeout-seconds", "0"], "Error"),
        (
            vec!["--workspace-volume-size-gb", "1", "--snapshot-poll-interval-secs", "0"],
            "Error",
        ),
        (
            vec!["--workspace-volume-size-gb", "1", "--warm-pool-size", "1"],
            "Error",
        ),
        (
            [&volume[..], &["--max-leases", "1", "--workspace-volume-size-gb", "1"]].concat(),
            "Error",
        ),
        (
            vec![
                "--volume",
                "bad",
                "--volume-tenant",
                "tenant-a",
                "--volume-drive",
                "data",
            ],
            "Error",
        ),
        (
            vec![
                "--volume",
                "vol_test",
                "--volume-tenant",
                "bad tenant!",
                "--volume-drive",
                "data",
            ],
            "Error",
        ),
        (
            vec![
                "--volume",
                "vol_test",
                "--volume-tenant",
                "tenant-a",
                "--volume-drive",
                "bad drive!",
            ],
            "Error",
        ),
        ([&volume[..], &["--volume-path", "relative"]].concat(), "Error"),
        ([&volume[..], &["--max-leases", "2"]].concat(), "Error"),
        (vec!["--memory-mib", "0"], "invalid value"),
        (vec!["--volume-tenant", "tenant-a"], "required arguments"),
    ];
    for (arguments, message) in cases {
        let mut command = worker(UNUSED_NATS, UNUSED_ENDPOINT, dir.path());
        command.args(&arguments);
        let (output, _) = finish(command).await?;
        let stderr = stderr_of(&output);
        assert!(!output.status.success(), "{arguments:?} succeeded");
        assert!(stderr.contains(message), "{arguments:?}: {stderr}");
    }
    Ok(())
}

#[tokio::test]
async fn an_empty_token_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut command = worker(UNUSED_NATS, UNUSED_ENDPOINT, dir.path());
    command.env("FLY_API_TOKEN", "");
    command.args(["--token", ""]);
    rejected(command, "Error").await
}

#[tokio::test]
async fn a_token_that_is_not_a_header_value_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut command = worker(UNUSED_NATS, UNUSED_ENDPOINT, dir.path());
    command.args(["--token", "line\nbreak"]);
    rejected(command, "Error").await
}

#[tokio::test]
async fn an_unreachable_nats_server_fails_quickly() -> TestResult {
    let fly = Fly::start().await;
    let dir = tempfile::tempdir()?;
    let command = worker(UNUSED_NATS, &fly.url(), dir.path());
    let (output, elapsed) = finish(command).await?;
    let stderr = stderr_of(&output);
    assert!(!output.status.success());
    assert!(stderr.contains("connecting to NATS"), "{stderr}");
    assert!(elapsed < Duration::from_secs(30), "{elapsed:?}");
    Ok(())
}

#[tokio::test]
async fn a_worker_id_that_differs_from_the_durable_identity_is_refused() -> TestResult {
    let fly = Fly::start().await;
    let dir = tempfile::tempdir()?;
    let mut command = worker(UNUSED_NATS, &fly.url(), dir.path());
    command.args(["--worker-id", "someone-else"]);
    rejected(command, "configured worker id differs from its durable identity").await
}

#[tokio::test]
async fn an_unreachable_fly_api_is_refused_on_start() -> TestResult {
    let dir = tempfile::tempdir()?;
    let command = worker(UNUSED_NATS, UNUSED_ENDPOINT, dir.path());
    let (output, _) = finish(command).await?;
    assert!(!output.status.success());
    assert!(!stderr_of(&output).is_empty());
    Ok(())
}

#[tokio::test]
async fn an_invalid_authority_mode_is_refused() -> TestResult {
    let nats = Nats::start().await;
    let fly = Fly::start().await;
    let dir = tempfile::tempdir()?;
    let mut command = worker(&nats.url(), &fly.url(), dir.path());
    command.env("SANDBOX_WORKER_AUTHORITY", "sometimes");
    rejected(command, "sometimes").await
}
