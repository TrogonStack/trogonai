use std::process::Stdio;
use std::time::Duration;

use futures::StreamExt;
use tokio::process::Command;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_protocol::subjects::worker_heartbeat_filter;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const WAIT: Duration = Duration::from_secs(30);

#[tokio::test]
async fn the_binary_serves_until_interrupted() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let mut heartbeats = server
        .client()
        .await
        .subscribe(worker_heartbeat_filter(ProviderKind::AzureContainerApps))
        .await?;
    let mut worker = Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-aca-worker"))
        .args([
            "--nats-url",
            server.address(),
            "--pool-management-endpoint",
            "https://pool.example/management",
            "--session-pool-name",
            "pool",
            "--metrics-addr",
            "127.0.0.1:0",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;

    tokio::time::timeout(WAIT, heartbeats.next())
        .await?
        .ok_or("no heartbeat")?;
    let pid = worker.id().ok_or("the worker already exited")?;
    let signalled = Command::new("kill").args(["-INT", &pid.to_string()]).status().await?;
    let status = tokio::time::timeout(WAIT, worker.wait()).await??;

    assert!(signalled.success());
    assert!(status.success());
    Ok(())
}
