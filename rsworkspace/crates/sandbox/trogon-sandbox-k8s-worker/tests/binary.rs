mod support;

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use futures::StreamExt;
use serde_json::{Value, json};
use support::*;
use tokio::process::{Child, Command};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_protocol::{nats_transfer, subjects};

const BINARY: &str = env!("CARGO_BIN_EXE_trogon-sandbox-k8s-worker");
const UNUSED_NATS: &str = "nats://127.0.0.1:9";
/// Settings the binary reads that a test must not inherit from the machine running it.
const KNOBS: [&str; 26] = [
    "KUBECONFIG",
    "KUBERNETES_SERVICE_HOST",
    "SANDBOX_DISPATCH_FENCE_FILE",
    "SANDBOX_NATS_URL",
    "SANDBOX_K8S_NAMESPACE",
    "SANDBOX_IMAGE",
    "SANDBOX_POD_RUNTIME_CLASS",
    "SANDBOX_POD_RUNTIME_CLASS_MICRO_VM",
    "SANDBOX_POD_RUNTIME_CLASS_USER_SPACE_KERNEL",
    "SANDBOX_SERVICE_ACCOUNT",
    "SANDBOX_K8S_RESOURCE_NAMING",
    "SANDBOX_IMAGE_PULL_SECRETS",
    "SANDBOX_WORKER_MAX_CONCURRENT",
    "SANDBOX_SNAPSHOT_REGISTRY",
    "SANDBOX_OCI_INSECURE_REGISTRIES",
    "SANDBOX_COSIGN_PUBLIC_KEY",
    "SANDBOX_DIGEST_CACHE_SIZE",
    "SANDBOX_SNAPSHOT_REPOSITORY",
    "SANDBOX_K8S_WORKER_IDENTITY",
    "SANDBOX_K8S_PERSIST_ROLLBACK_STATE",
    "SANDBOX_WORKER_AUTHORITY",
    "SANDBOX_TERMINAL_REPLAY_BYTES",
    "SANDBOX_OTLP_ENDPOINT",
    "SANDBOX_METRICS_ADDR",
    "SANDBOX_K8S_HONOR_IMAGE_USER",
    "SANDBOX_K8S_RECOVER_UNLABELED_LEASES",
];
const PROCESS_LIMIT: Duration = Duration::from_secs(90);

fn kubeconfig(directory: &Path, endpoint: &str) -> Boxed<PathBuf> {
    let path = directory.join("kubeconfig");
    let body = format!(
        "apiVersion: v1\nkind: Config\nclusters:\n- name: fake\n  cluster:\n    server: {endpoint}\n\
         contexts:\n- name: fake\n  context:\n    cluster: fake\n    user: fake\n    namespace: {NAMESPACE}\n\
         current-context: fake\nusers:\n- name: fake\n  user: {{}}\n"
    );
    std::fs::write(&path, body)?;
    Ok(path)
}

/// The binary with the required settings, the environment it would otherwise inherit cleared of its own knobs.
fn worker(nats_url: &str, kubeconfig: &Path) -> Command {
    let mut command = Command::new(BINARY);
    for name in KNOBS {
        command.env_remove(name);
    }
    command
        .env("RUST_LOG", "off")
        .env("KUBECONFIG", kubeconfig)
        .env("SANDBOX_NATS_URL", nats_url)
        .env("SANDBOX_K8S_NAMESPACE", NAMESPACE)
        .env("SANDBOX_IMAGE", IMAGE)
        .env("SANDBOX_K8S_WORKER_IDENTITY", "worker-binary")
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

/// Starts the binary and waits for its first heartbeat.
async fn serve(command: &mut Command, nats: &JetStreamTestServer) -> Boxed<(Child, Value)> {
    let client = nats.client().await;
    let mut beats = client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::K8sMicroVm))
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
async fn a_full_run_serves_leases_and_stops_on_interrupt() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let cluster = Cluster::start().await?;
    let directory = tempfile::tempdir()?;
    let config = kubeconfig(directory.path(), cluster.endpoint())?;
    let mut command = worker(&format!("nats://{}", nats.address()), &config);
    command.env("SANDBOX_WORKER_MAX_CONCURRENT", "3");
    let (child, beat) = serve(&mut command, &nats).await?;
    assert_eq!(beat["capacity"]["max_concurrent_leases"], 3, "{beat}");

    // The worker subscribes after its first heartbeat.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let client = nats.client().await;
    let payload = json!({"lease_id": "lease-binary", "request": serde_json::to_value(lease_request()?)?});
    let reply = nats_transfer::request(
        &client,
        &subjects::provider_dispatch(ProviderKind::K8sMicroVm),
        serde_json::to_vec(&payload)?.into(),
        None,
        Duration::from_secs(30),
    )
    .await?;
    let reply: Value = serde_json::from_slice(&reply)?;
    assert!(
        reply.get("provider_error").is_none() && reply.get("decode_error").is_none(),
        "{reply}"
    );
    assert_eq!(cluster.state().jobs_created, 1);
    let status = stop(child).await?;
    assert!(status.success(), "{status}");
    Ok(())
}

#[tokio::test]
async fn a_run_with_insecure_registries_and_persisted_rollback_state_serves() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let cluster = Cluster::start().await?;
    let directory = tempfile::tempdir()?;
    let config = kubeconfig(directory.path(), cluster.endpoint())?;
    let mut command = worker(&format!("nats://{}", nats.address()), &config);
    command
        .env("SANDBOX_OCI_INSECURE_REGISTRIES", "registry.local:5000,")
        .env("SANDBOX_K8S_PERSIST_ROLLBACK_STATE", "true");
    let (child, _) = serve(&mut command, &nats).await?;
    let status = stop(child).await?;
    assert!(status.success(), "{status}");
    Ok(())
}

#[tokio::test]
async fn a_missing_namespace_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let config = kubeconfig(directory.path(), "http://127.0.0.1:9")?;
    let mut command = worker(UNUSED_NATS, &config);
    command.env_remove("SANDBOX_K8S_NAMESPACE");
    rejected(command, "--k8s-namespace").await
}

#[tokio::test]
async fn an_invalid_worker_identity_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let config = kubeconfig(directory.path(), "http://127.0.0.1:9")?;
    let mut command = worker(UNUSED_NATS, &config);
    command.env("SANDBOX_K8S_WORKER_IDENTITY", "-not valid-");
    rejected(command, "not a valid Kubernetes label value").await
}

#[tokio::test]
async fn an_unreachable_nats_server_fails_quickly() -> TestResult {
    let directory = tempfile::tempdir()?;
    let config = kubeconfig(directory.path(), "http://127.0.0.1:9")?;
    rejected(worker(UNUSED_NATS, &config), "Connect {").await
}

#[tokio::test]
async fn a_malformed_insecure_registry_is_refused() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let directory = tempfile::tempdir()?;
    let config = kubeconfig(directory.path(), "http://127.0.0.1:9")?;
    let mut command = worker(&format!("nats://{}", nats.address()), &config);
    command.env("SANDBOX_OCI_INSECURE_REGISTRIES", "not a host!");
    let output = finish(command).await?;
    assert!(!output.status.success());
    Ok(())
}

#[tokio::test]
async fn an_unreadable_cosign_key_is_refused() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let cluster = Cluster::start().await?;
    let directory = tempfile::tempdir()?;
    let config = kubeconfig(directory.path(), cluster.endpoint())?;
    let mut command = worker(&format!("nats://{}", nats.address()), &config);
    command.env("SANDBOX_COSIGN_PUBLIC_KEY", directory.path().join("absent.pem"));
    rejected(command, "Cosign(PublicKey").await
}

#[tokio::test]
async fn an_invalid_authority_mode_is_refused() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let cluster = Cluster::start().await?;
    let directory = tempfile::tempdir()?;
    let config = kubeconfig(directory.path(), cluster.endpoint())?;
    let mut command = worker(&format!("nats://{}", nats.address()), &config);
    command.env("SANDBOX_WORKER_AUTHORITY", "sometimes");
    rejected(command, "sometimes").await
}

#[tokio::test]
async fn a_cluster_that_cannot_be_read_stops_recovery() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let cluster = Cluster::start().await?;
    cluster.state().forbid_reads.push("jobs".into());
    let directory = tempfile::tempdir()?;
    let config = kubeconfig(directory.path(), cluster.endpoint())?;
    rejected(worker(&format!("nats://{}", nats.address()), &config), "Recovery").await
}
