use axum::Router;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{CapabilitySupport, ProviderKind, WorkerHeartbeat, WorkerId};
use trogon_sandbox_protocol::subjects;

use super::super::{build_heartbeat, spawn_heartbeat_task};
use super::support::{TestResult, WAIT, provider_holding};

#[test]
fn the_heartbeat_reports_the_configured_cap_and_the_live_count() -> TestResult {
    let heartbeat = build_heartbeat(&WorkerId::new("worker-1")?, 64, 3);
    assert_eq!(heartbeat.provider, ProviderKind::Substrate);
    assert_eq!(heartbeat.capacity.max_concurrent_leases, 64);
    assert_eq!(heartbeat.capacity.current_leases, 3);
    assert!(heartbeat.cached_digests.is_empty());
    assert_eq!(heartbeat.profile.snapshot.can_snapshot, CapabilitySupport::Unsupported);
    Ok(())
}

#[tokio::test]
async fn the_heartbeat_task_publishes_the_providers_live_lease_count() -> TestResult {
    let server = trogon_nats::test_support::JetStreamTestServer::start().await;
    let client = server.client().await;
    let (provider, _control, _index) = provider_holding(Router::new(), &["01J0", "01J1"]).await?;
    let mut beats = client
        .subscribe(subjects::worker_heartbeat(
            ProviderKind::Substrate,
            provider.worker_id(),
        ))
        .await?;
    client.flush().await?;
    let cancel = CancellationToken::new();
    spawn_heartbeat_task(client.clone(), provider.clone(), cancel.clone());

    let beat = tokio::time::timeout(WAIT, beats.next()).await?.ok_or("no heartbeat")?;
    cancel.cancel();

    let heartbeat: WorkerHeartbeat = serde_json::from_slice(&beat.payload)?;
    assert_eq!(&heartbeat.worker_id, provider.worker_id());
    assert_eq!(heartbeat.capacity.current_leases, 2);
    assert_eq!(
        heartbeat.capacity.max_concurrent_leases,
        provider.config().max_concurrent_leases
    );
    assert_eq!(heartbeat.supports_lease_epoch, CapabilitySupport::Supported);
    Ok(())
}
