mod nats_common;

use std::time::Duration;

use clap::Parser;
use trogon_sandbox_memory_worker::{MemoryWorkerArgs, RunError, run};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn run_serves_until_asked_to_stop() -> TestResult {
    let server = nats_common::Server::start().await;
    let client = server.client().await?;
    let args = MemoryWorkerArgs::parse_from([
        "worker",
        "--nats-url",
        &server.url(),
        "--regions",
        "eu-1,us-1",
        "--max-concurrent-leases",
        "3",
    ]);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serving = tokio::spawn(async move {
        run(args, async {
            let _ = stopped.await;
        })
        .await
    });
    let granted = tokio::time::timeout(
        Duration::from_secs(10),
        client.request("sandbox.provider.mock.lease", bytes::Bytes::from_static(b"{}")),
    )
    .await;
    assert!(granted.is_ok());
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = stop.send(());
    serving.await??;
    Ok(())
}

#[tokio::test]
async fn run_reports_an_unreachable_server() -> TestResult {
    let args = MemoryWorkerArgs::parse_from(["worker", "--nats-url", "nats://127.0.0.1:1"]);
    let outcome = run(args, std::future::pending()).await;
    assert!(matches!(outcome, Err(RunError::Connect { ref url, .. }) if url == "nats://127.0.0.1:1"));
    Ok(())
}
