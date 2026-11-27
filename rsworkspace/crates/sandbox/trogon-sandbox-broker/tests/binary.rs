//! Runs the broker binary: bad flags and a missing configuration are refused
//! with a non-zero exit, and a started broker shuts down cleanly on SIGTERM.

use std::error::Error;
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_protocol::subjects::CAPABILITY_LIST_REQUEST;

type TestResult = Result<(), Box<dyn Error>>;

const ANSWER_ATTEMPTS: usize = 80;
const EXIT_TIMEOUT: Duration = Duration::from_secs(30);

fn broker() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-broker"));
    command
        .env_remove("SPICEDB_ENDPOINT")
        .env_remove("SPICEDB_TOKEN")
        .env_remove("SANDBOX_AUTHZ_ENFORCE")
        .env_remove("SANDBOX_INSECURE_DEV")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

async fn wait_for_exit(mut child: std::process::Child) -> Result<ExitStatus, Box<dyn Error>> {
    let deadline = tokio::time::Instant::now() + EXIT_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if tokio::time::Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Err("the broker did not exit in time".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn an_unknown_flag_is_refused() -> TestResult {
    let child = broker().arg("--no-such-flag").spawn()?;
    let status = wait_for_exit(child).await?;
    assert!(!status.success());
    Ok(())
}

#[tokio::test]
async fn an_invalid_flag_value_is_refused() -> TestResult {
    let child = broker().args(["--default-cpu-cap", "many"]).spawn()?;
    let status = wait_for_exit(child).await?;
    assert!(!status.success());
    Ok(())
}

#[tokio::test]
async fn help_exits_successfully() -> TestResult {
    let child = broker().arg("--help").spawn()?;
    let status = wait_for_exit(child).await?;
    assert!(status.success());
    Ok(())
}

#[tokio::test]
async fn enforced_authz_without_spicedb_is_refused() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let child = broker()
        .env("NATS_URL", format!("nats://{}", server.address()))
        .env("SANDBOX_JETSTREAM_REPLICAS", "1")
        .env("SANDBOX_METRICS_ADDR", "127.0.0.1:0")
        .spawn()?;
    let status = wait_for_exit(child).await?;
    assert!(!status.success());
    Ok(())
}

#[tokio::test]
async fn an_unreachable_nats_server_is_refused() -> TestResult {
    let child = broker()
        .env("NATS_URL", "nats://127.0.0.1:1")
        .arg("--insecure-dev")
        .env("SANDBOX_METRICS_ADDR", "127.0.0.1:0")
        .spawn()?;
    let status = wait_for_exit(child).await?;
    assert!(!status.success());
    Ok(())
}

#[tokio::test]
async fn a_running_broker_shuts_down_on_sigterm() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let child = broker()
        .env("NATS_URL", format!("nats://{}", server.address()))
        .env("SANDBOX_JETSTREAM_REPLICAS", "1")
        .env("SANDBOX_METRICS_ADDR", "127.0.0.1:0")
        .arg("--insecure-dev")
        .spawn()?;
    let pid = child.id().to_string();

    let mut answered = false;
    for _ in 0..ANSWER_ATTEMPTS {
        let request = client.request(CAPABILITY_LIST_REQUEST.to_string(), "{}".into());
        if let Ok(Ok(_)) = tokio::time::timeout(Duration::from_secs(1), request).await {
            answered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(answered, "the broker never answered a request");

    let signalled = Command::new("kill").args(["-TERM", &pid]).status()?;
    assert!(signalled.success());
    let status = wait_for_exit(child).await?;
    assert!(status.success(), "the broker exited with {status}");
    Ok(())
}
