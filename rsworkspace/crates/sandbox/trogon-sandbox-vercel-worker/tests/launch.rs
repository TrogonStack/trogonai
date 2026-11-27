mod support;

use std::time::Duration;

use clap::Parser;
use futures::StreamExt;
use serde_json::Value;
use support::TestResult;
use tokio::sync::oneshot;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_protocol::subjects;
use trogon_sandbox_vercel_worker::{VercelWorkerError, WorkerArgs, launch};

const UNUSED_NATS: &str = "nats://127.0.0.1:9";

fn args(nats_url: &str, extra: &[&str]) -> Result<WorkerArgs, clap::Error> {
    let mut argv = vec![
        "trogon-sandbox-vercel-worker",
        "--token",
        "test-token",
        "--team-id",
        "team_1",
        "--project-id",
        "prj_1",
        "--api-url",
        "http://127.0.0.1:9",
        "--nats-url",
        nats_url,
    ];
    argv.extend_from_slice(extra);
    WorkerArgs::try_parse_from(argv)
}

#[tokio::test]
async fn an_unknown_runtime_is_refused_before_connecting() -> TestResult {
    let args = args(UNUSED_NATS, &["--runtime", "node18"])?;
    let result = launch(&args, std::future::pending()).await;
    assert!(matches!(result, Err(VercelWorkerError::Runtime(_))));
    Ok(())
}

#[tokio::test]
async fn an_unreachable_nats_server_is_a_connect_error() -> TestResult {
    let args = args(UNUSED_NATS, &[])?;
    let result = tokio::time::timeout(Duration::from_secs(30), launch(&args, std::future::pending())).await?;
    assert!(matches!(result, Err(VercelWorkerError::Connect(_))));
    Ok(())
}

#[tokio::test]
async fn a_running_worker_heartbeats_and_stops_when_told_to() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let client = nats.client().await;
    let mut beats = client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::Vercel))
        .await?;
    client.flush().await?;
    let url = format!("nats://{}", nats.address());
    let args = args(&url, &["--plan", "hobby", "--max-vcpus", "16", "--regions", "iad1"])?;
    let (stop, stopped) = oneshot::channel::<()>();
    let worker = tokio::spawn(async move {
        launch(&args, async {
            let _ = stopped.await;
        })
        .await
    });
    let message = tokio::time::timeout(Duration::from_secs(60), beats.next())
        .await?
        .ok_or("no heartbeat")?;
    let beat: Value = serde_json::from_slice(&message.payload)?;
    assert_eq!(beat["capacity"]["max_concurrent_leases"], 4);
    assert_eq!(beat["profile"]["placement"]["regions"][0], "iad1");
    assert_eq!(beat["profile"]["resources"]["vcpu_millis"]["max"], 4000);
    assert_eq!(beat["profile"]["lifecycle"]["max_lease_lifetime_seconds"], 2700);
    stop.send(()).map_err(|()| "the worker is gone")?;
    tokio::time::timeout(Duration::from_secs(60), worker).await???;
    Ok(())
}
