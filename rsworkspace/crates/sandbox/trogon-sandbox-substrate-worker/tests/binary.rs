use std::process::Stdio;
use std::time::Duration;

use futures::StreamExt;
use tokio::process::Command;
use tonic::service::Routes;
use tonic::transport::Server;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_protocol::subjects::worker_heartbeat_filter;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const WAIT: Duration = Duration::from_secs(60);

#[tokio::test]
async fn the_binary_serves_until_interrupted() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let address = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?;
    let control = format!("http://{address}");
    // A control plane with no services: every call is refused, so recovery logs and the
    // worker starts with an empty active map.
    let control_plane = tokio::spawn(async move {
        let _ = Server::builder().add_routes(Routes::default()).serve(address).await;
    });

    let mut heartbeats = nats
        .client()
        .await
        .subscribe(worker_heartbeat_filter(ProviderKind::Substrate))
        .await?;
    let mut worker = Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-substrate-worker"))
        .args([
            "--nats-url",
            nats.address(),
            "--ate-api-endpoint",
            &control,
            "--allow-insecure-router",
            "--agent-image",
            "registry.example/agent@sha256:0000000000000000000000000000000000000000000000000000000000000000",
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
    control_plane.abort();

    assert!(signalled.success());
    assert!(status.success());
    Ok(())
}
