use std::process::Output;
use std::time::Duration;

use async_nats::{Request, RequestErrorKind};
use bytes::Bytes;
use tokio::process::{Child, Command};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_protocol::subjects::lease_released;

type Boxed<T> = Result<T, Box<dyn std::error::Error>>;
type TestResult = Boxed<()>;

const BINARY: &str = env!("CARGO_BIN_EXE_trogon-sandbox-snapshot-builder");
const UNUSED_NATS: &str = "nats://127.0.0.1:9";
/// Settings the binary reads that a test must not inherit from the machine running it.
const KNOBS: [&str; 7] = [
    "SANDBOX_NATS_URL",
    "SANDBOX_SNAPSHOT_BUILDER_MAX_CONCURRENT",
    "SANDBOX_REGISTRY_URL",
    "SANDBOX_COSIGN_KMS_URI",
    "SANDBOX_SCAN_CVSS_THRESHOLD",
    "SANDBOX_REQUIRE_SCAN_PASS",
    "SANDBOX_OCI_INSECURE_REGISTRIES",
];
const PROCESS_LIMIT: Duration = Duration::from_secs(60);

fn builder(nats_url: &str) -> Command {
    let mut command = Command::new(BINARY);
    for name in KNOBS {
        command.env_remove(name);
    }
    command
        .env("RUST_LOG", "off")
        .env("SANDBOX_NATS_URL", nats_url)
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
    let pid = child.id().ok_or("the builder already exited")?;
    let status = std::process::Command::new("kill")
        .args(["-INT", &pid.to_string()])
        .status()?;
    assert!(status.success());
    Ok(())
}

#[tokio::test]
async fn help_describes_the_service_and_exits_cleanly() -> TestResult {
    let mut command = builder(UNUSED_NATS);
    command.arg("--help");
    let output = finish(command).await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("--nats-url"), "{stdout}");
    assert!(stdout.contains("--require-scan-pass"), "{stdout}");
    Ok(())
}

#[tokio::test]
async fn an_unknown_flag_is_refused() -> TestResult {
    let mut command = builder(UNUSED_NATS);
    command.arg("--no-such-flag");
    let output = finish(command).await?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--no-such-flag"));
    Ok(())
}

#[tokio::test]
async fn a_malformed_insecure_registry_is_refused() -> TestResult {
    let mut command = builder(UNUSED_NATS);
    command.env("SANDBOX_OCI_INSECURE_REGISTRIES", "not a host!");
    let output = finish(command).await?;
    assert!(!output.status.success());
    Ok(())
}

#[tokio::test]
async fn an_unreachable_nats_server_fails_quickly() -> TestResult {
    let output = finish(builder(UNUSED_NATS)).await?;
    assert!(!output.status.success());
    Ok(())
}

#[tokio::test]
async fn a_running_builder_listens_for_releases_and_stops_on_interrupt() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let client = nats.client().await;
    let mut command = builder(&format!("nats://{}", nats.address()));
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let subject = lease_released(&LeaseId::new("binary-probe"));
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let request = Request::new()
                .payload(Bytes::from_static(b"{}"))
                .timeout(Some(Duration::from_millis(50)));
            match client.send_request(subject.clone(), request).await {
                Err(error) if error.kind() == RequestErrorKind::TimedOut => return Ok(()),
                Err(error) if error.kind() == RequestErrorKind::NoResponders => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                other => return Err(format!("unexpected probe result: {other:?}")),
            }
        }
    })
    .await??;
    interrupt(&child)?;
    let status = tokio::time::timeout(PROCESS_LIMIT, child.wait()).await??;
    assert!(status.success(), "{status}");
    Ok(())
}
