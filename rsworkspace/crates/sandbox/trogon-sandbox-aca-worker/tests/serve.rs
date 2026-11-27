use std::time::Duration;

use clap::Parser;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_aca_worker::{NatsLifecycleEmitter, ServeError, WorkerArgs, serve};
use trogon_sandbox_contract::{LifecycleEvent, ProviderKind, SandboxId, WorkerHeartbeat};
use trogon_sandbox_protocol::LifecycleEmitter;
use trogon_sandbox_protocol::subjects::worker_heartbeat_filter;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const WAIT: Duration = Duration::from_secs(15);

fn args(url: &str) -> Result<WorkerArgs, clap::Error> {
    WorkerArgs::try_parse_from([
        "worker",
        "--nats-url",
        url,
        "--max-concurrent-leases",
        "3",
        "--pool-management-endpoint",
        "https://pool.example/management",
        "--session-pool-name",
        "pool",
        "--region",
        "westeurope",
    ])
}

#[tokio::test]
async fn serving_publishes_heartbeats_until_cancelled() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let mut heartbeats = server
        .client()
        .await
        .subscribe(worker_heartbeat_filter(ProviderKind::AzureContainerApps))
        .await?;
    let cancel = CancellationToken::new();
    let serving = tokio::spawn({
        let args = args(server.address())?;
        let cancel = cancel.clone();
        async move { serve(&args, cancel).await }
    });

    let message = tokio::time::timeout(WAIT, heartbeats.next())
        .await?
        .ok_or("no heartbeat")?;
    let heartbeat: WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
    cancel.cancel();
    tokio::time::timeout(WAIT, serving).await???;

    assert_eq!(heartbeat.provider, ProviderKind::AzureContainerApps);
    assert_eq!(heartbeat.capacity.max_concurrent_leases, 3);
    assert_eq!(heartbeat.capacity.current_leases, 0);
    assert_eq!(heartbeat.profile.placement.regions, vec!["westeurope".to_owned()]);
    Ok(())
}

#[tokio::test]
async fn an_unreachable_server_is_a_connect_error() -> TestResult {
    let port = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();

    let result = serve(&args(&format!("nats://127.0.0.1:{port}"))?, CancellationToken::new()).await;

    assert!(matches!(result, Err(ServeError::Connect { .. })));
    Ok(())
}

#[tokio::test]
async fn lifecycle_events_are_published_to_their_subject() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let mut events = server.client().await.subscribe("lifecycle.test").await?;
    let emitter = NatsLifecycleEmitter::new(server.jetstream().await, "lifecycle.test".to_owned());
    let started = LifecycleEvent::Started {
        sandbox_id: SandboxId::new("sandbox-1")?,
    };

    emitter.emit(started.clone()).await;

    let message = tokio::time::timeout(WAIT, events.next()).await?.ok_or("no event")?;
    assert_eq!(serde_json::from_slice::<LifecycleEvent>(&message.payload)?, started);
    Ok(())
}

#[tokio::test]
async fn an_event_too_large_to_publish_is_dropped() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let emitter = NatsLifecycleEmitter::new(server.jetstream().await, "lifecycle.large".to_owned());
    let oversized = LifecycleEvent::Failed {
        sandbox_id: SandboxId::new("sandbox-1")?,
        error: "x".repeat(4 * 1024 * 1024),
    };

    emitter.emit(oversized).await;
    Ok(())
}
