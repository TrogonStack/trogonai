use std::time::Duration;

use clap::Parser;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::oneshot;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_protocol::subjects;
use trogon_std::env::{InMemoryEnv, SystemEnv};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const UNUSED_NATS: &str = "nats://127.0.0.1:9";
const STEP: Duration = Duration::from_secs(60);

fn args(nats_url: &str, extra: &[&str]) -> Result<WorkerArgs, clap::Error> {
    let mut argv = vec![
        "trogon-sandbox-e2b-worker",
        "--api-key",
        "test-key",
        "--api-url",
        "http://127.0.0.1:9",
        "--nats-url",
        nats_url,
    ];
    argv.extend_from_slice(extra);
    WorkerArgs::try_parse_from(argv)
}

#[tokio::test]
async fn an_unknown_authority_mode_is_refused() -> TestResult {
    let env = InMemoryEnv::new();
    env.set("SANDBOX_WORKER_AUTHORITY", "bogus");
    let result = launch(&args(UNUSED_NATS, &[])?, &env, std::future::pending()).await;
    assert!(matches!(result, Err(LaunchError::Authority(_))));
    Ok(())
}

#[tokio::test]
async fn an_unusable_domain_is_a_config_error() -> TestResult {
    let result = launch(
        &args(UNUSED_NATS, &["--domain", "not a domain"])?,
        &InMemoryEnv::new(),
        std::future::pending(),
    )
    .await;
    assert!(matches!(result, Err(LaunchError::Config(_))));
    Ok(())
}

#[tokio::test]
async fn an_insecure_envd_override_is_a_config_error() -> TestResult {
    let result = launch(
        &args(UNUSED_NATS, &["--envd-url", "http://example.com"])?,
        &InMemoryEnv::new(),
        std::future::pending(),
    )
    .await;
    assert!(matches!(result, Err(LaunchError::Config(_))));
    Ok(())
}

#[tokio::test]
async fn an_unreachable_nats_server_is_a_connect_error() -> TestResult {
    let result = tokio::time::timeout(
        STEP,
        launch(&args(UNUSED_NATS, &[])?, &InMemoryEnv::new(), std::future::pending()),
    )
    .await?;
    assert!(matches!(result, Err(LaunchError::Connect(_))));
    Ok(())
}

#[tokio::test]
async fn a_ledger_that_cannot_be_opened_is_a_ledger_error() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let url = format!("nats://{}", nats.address());
    let dir = tempfile::tempdir()?;
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"file")?;
    let ledger = blocker.join("ledger.json");
    let args = args(&url, &["--dispatch-ledger-file", ledger.to_str().ok_or("path")?])?;
    let result = tokio::time::timeout(STEP, launch(&args, &InMemoryEnv::new(), std::future::pending())).await?;
    assert!(matches!(result, Err(LaunchError::Ledger(_))));
    Ok(())
}

async fn first_heartbeat(extra: &[&str]) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let nats = JetStreamTestServer::start().await;
    let client = nats.client().await;
    let mut beats = client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::E2b))
        .await?;
    client.flush().await?;
    let url = format!("nats://{}", nats.address());
    let args = args(&url, extra)?;
    let (stop, stopped) = oneshot::channel::<()>();
    let worker = tokio::spawn(async move {
        launch(&args, &SystemEnv, async {
            let _ = stopped.await;
        })
        .await
    });
    let message = tokio::time::timeout(STEP, beats.next()).await?.ok_or("no heartbeat")?;
    let beat: Value = serde_json::from_slice(&message.payload)?;
    stop.send(()).map_err(|()| "the worker is gone")?;
    tokio::time::timeout(STEP, worker).await???;
    Ok(beat)
}

#[tokio::test]
async fn a_running_worker_heartbeats_and_stops_when_told_to() -> TestResult {
    let beat = first_heartbeat(&["--regions", "eu", "--max-concurrent-leases", "7"]).await?;
    assert_eq!(beat["capacity"]["max_concurrent_leases"], 7);
    assert_eq!(beat["profile"]["placement"]["regions"][0], "eu");
    assert_eq!(beat["dispatch_protocol"], "legacy");
    Ok(())
}

#[tokio::test]
async fn a_worker_with_a_ledger_takes_targeted_assignments() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = dir.path().join("dispatch").join("ledger.json");
    let beat = first_heartbeat(&["--dispatch-ledger-file", ledger.to_str().ok_or("path")?]).await?;
    assert_eq!(beat["dispatch_protocol"], "targeted_v1");
    Ok(())
}
