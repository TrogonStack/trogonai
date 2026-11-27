use std::time::Duration;

use clap::Parser;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{CapabilitySupport, LifecycleEvent, ProviderKind, SandboxId, WorkerHeartbeat, WorkerId};
use trogon_sandbox_protocol::LifecycleEmitter;
use trogon_sandbox_protocol::subjects::worker_heartbeat_filter;
use trogon_sandbox_v8_wasm_worker::{NatsLifecycleEmitter, ServeError, WorkerArgs, build_heartbeat, serve};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const WAIT: Duration = Duration::from_secs(15);

fn args(url: &str) -> Result<WorkerArgs, clap::Error> {
    WorkerArgs::try_parse_from(["worker", "--nats-url", url, "--max-concurrent-leases", "3"])
}

#[test]
fn the_defaults_point_at_a_local_server() -> TestResult {
    let args = WorkerArgs::try_parse_from(["worker"])?;
    assert_eq!(args.nats_url, "nats://127.0.0.1:4222");
    assert_eq!(args.max_concurrent_leases, 8);
    assert!(args.dispatch_fence_file.is_none());
    Ok(())
}

#[test]
fn the_flags_override_the_defaults() -> TestResult {
    let args = WorkerArgs::try_parse_from([
        "worker",
        "--nats-url",
        "nats://example.invalid:1",
        "--max-concurrent-leases",
        "2",
        "--dispatch-fence-file",
        "fence.journal",
    ])?;
    assert_eq!(args.nats_url, "nats://example.invalid:1");
    assert_eq!(args.max_concurrent_leases, 2);
    assert_eq!(
        args.dispatch_fence_file.as_deref(),
        Some(std::path::Path::new("fence.journal"))
    );
    Ok(())
}

#[test]
fn a_heartbeat_reports_the_provider_with_no_active_leases() -> TestResult {
    let heartbeat = build_heartbeat(&WorkerId::new("worker-1")?, 8);

    assert_eq!(heartbeat.worker_id, WorkerId::new("worker-1")?);
    assert_eq!(heartbeat.provider, ProviderKind::V8Wasm);
    assert_eq!(heartbeat.capacity.max_concurrent_leases, 8);
    assert_eq!(heartbeat.capacity.current_leases, 0);
    assert_eq!(heartbeat.supports_lease_epoch, CapabilitySupport::Unsupported);
    assert!(heartbeat.cached_digests.is_empty());
    Ok(())
}

#[tokio::test]
async fn serving_publishes_heartbeats_until_cancelled() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let mut heartbeats = server
        .client()
        .await
        .subscribe(worker_heartbeat_filter(ProviderKind::V8Wasm))
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

    assert_eq!(heartbeat.provider, ProviderKind::V8Wasm);
    assert_eq!(heartbeat.capacity.max_concurrent_leases, 3);
    assert_eq!(heartbeat.capacity.current_leases, 0);
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
