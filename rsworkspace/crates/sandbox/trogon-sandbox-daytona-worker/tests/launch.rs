mod support;

use std::time::Duration;

use clap::Parser;
use futures::StreamExt;
use serde_json::Value;
use support::{TestResult, key};
use tokio::sync::oneshot;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_daytona_worker::{DaytonaWorkerError, WorkerArgs, launch};
use trogon_sandbox_protocol::subjects;

const UNUSED_NATS: &str = "nats://127.0.0.1:9";

fn args(nats_url: &str, extra: &[&str]) -> Result<WorkerArgs, clap::Error> {
    let api_key = key();
    let mut argv = vec![
        "trogon-sandbox-daytona-worker",
        "--api-key",
        api_key.as_str(),
        "--api-url",
        "http://127.0.0.1:9",
        "--nats-url",
        nats_url,
    ];
    argv.extend_from_slice(extra);
    WorkerArgs::try_parse_from(argv)
}

#[tokio::test]
async fn an_unreachable_nats_server_is_a_connect_error() -> TestResult {
    let args = args(UNUSED_NATS, &[])?;
    let result = tokio::time::timeout(Duration::from_secs(30), launch(&args, std::future::pending())).await?;
    assert!(matches!(result, Err(DaytonaWorkerError::Connect(_))));
    Ok(())
}

#[tokio::test]
async fn a_running_worker_heartbeats_and_stops_when_told_to() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let client = nats.client().await;
    let mut beats = client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::Daytona))
        .await?;
    client.flush().await?;
    let url = format!("nats://{}", nats.address());
    let args = args(
        &url,
        &[
            "--max-concurrent-leases",
            "3",
            "--organization-id",
            "org-1",
            "--snapshot",
            "base:2",
            "--start-timeout-secs",
            "30",
        ],
    )?;
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
    assert_eq!(beat["capacity"]["max_concurrent_leases"], 3);
    assert_eq!(beat["capacity"]["current_leases"], 0);
    stop.send(()).map_err(|()| "the worker is gone")?;
    tokio::time::timeout(Duration::from_secs(60), worker).await???;
    Ok(())
}

#[tokio::test]
async fn settings_the_worker_cannot_use_are_refused_at_parse_time() -> TestResult {
    let cases: [&[&str]; 6] = [
        &["--http-timeout-secs", "0"],
        &["--start-timeout-secs", "0"],
        &["--start-timeout-secs", "3601"],
        &["--api-url", "http://example.com"],
        &["--organization-id", ""],
        &["--max-concurrent-leases", "many"],
    ];
    for extra in cases {
        assert!(args(UNUSED_NATS, extra).is_err(), "{extra:?}");
    }
    assert!(args(UNUSED_NATS, &["--api-key", ""]).is_err());
    Ok(())
}

#[tokio::test]
async fn a_supervisor_that_does_not_match_its_pin_stops_the_launch() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let dir = tempfile::tempdir()?;
    let artifact = dir.path().join("supervisor");
    std::fs::write(&artifact, b"not the pinned artifact")?;
    let path = artifact.to_string_lossy().into_owned();
    let pin = "0".repeat(64);
    let url = format!("nats://{}", nats.address());
    let args = args(
        &url,
        &["--command-supervisor", &path, "--command-supervisor-sha256", &pin],
    )?;
    let result = tokio::time::timeout(Duration::from_secs(30), launch(&args, std::future::pending())).await?;
    assert!(matches!(result, Err(DaytonaWorkerError::Runtime(_))));
    Ok(())
}
