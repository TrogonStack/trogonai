mod provider_profile_fixture;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime};

use tokio_util::sync::CancellationToken;
use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};
use trogon_sandbox_contract::{
    CapabilitySupport, ProviderKind, WorkerCapacity, WorkerDispatchProtocol, WorkerHeartbeat, WorkerId,
};
use trogon_sandbox_protocol::{DEFAULT_HEARTBEAT_INTERVAL, spawn_heartbeat_loop, subjects};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn heartbeat(worker_id: &WorkerId, current_leases: u32) -> WorkerHeartbeat {
    WorkerHeartbeat {
        supports_lease_epoch: CapabilitySupport::Unsupported,
        worker_id: worker_id.clone(),
        provider: ProviderKind::Mock,
        capacity: WorkerCapacity {
            max_concurrent_leases: 4,
            current_leases,
            capabilities: Default::default(),
        },
        emitted_at: SystemTime::now(),
        cached_digests: vec![],
        profile: provider_profile_fixture::mock_profile(CapabilitySupport::Supported),
        warm_pool_population: Default::default(),
        dispatch_protocol: WorkerDispatchProtocol::TargetedV1,
        native_volume_bindings: Default::default(),
    }
}

fn spawn(client: MockNatsClient, worker_id: &WorkerId, cancel: &CancellationToken, counter: &Arc<AtomicU32>) {
    let (id, counter) = (worker_id.clone(), counter.clone());
    spawn_heartbeat_loop(
        client,
        ProviderKind::Mock,
        worker_id.clone(),
        Duration::from_millis(20),
        cancel.clone(),
        move || {
            let (id, counter) = (id.clone(), counter.clone());
            async move { heartbeat(&id, counter.fetch_add(1, Ordering::SeqCst)) }
        },
    );
}

#[test]
fn the_default_interval_is_ten_seconds() {
    assert_eq!(DEFAULT_HEARTBEAT_INTERVAL, Duration::from_secs(10));
}

#[tokio::test(start_paused = true)]
async fn publishes_a_fresh_heartbeat_on_every_tick() -> TestResult {
    let client = MockNatsClient::new();
    let worker_id = WorkerId::new("hb-tick")?;
    let cancel = CancellationToken::new();
    let counter = Arc::new(AtomicU32::new(0));
    spawn(client.clone(), &worker_id, &cancel, &counter);
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(1)).await;

    let subjects_seen = client.published_messages();
    assert!(subjects_seen.len() >= 3);
    let expected = subjects::worker_heartbeat(ProviderKind::Mock, &worker_id);
    assert!(subjects_seen.iter().all(|subject| *subject == expected));
    let mut seen = Vec::new();
    for payload in client.published_payloads() {
        let heartbeat: WorkerHeartbeat = serde_json::from_slice(&payload)?;
        assert_eq!(heartbeat.worker_id, worker_id);
        assert_eq!(
            heartbeat.supports_lease_epoch,
            CapabilitySupport::Supported,
            "the loop advertises lease epochs even when the builder does not"
        );
        seen.push(heartbeat.capacity.current_leases);
    }
    assert_eq!(seen[..3], [0, 1, 2]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn stops_building_once_cancelled() -> TestResult {
    let client = MockNatsClient::new();
    let worker_id = WorkerId::new("hb-cancel")?;
    let cancel = CancellationToken::new();
    let counter = Arc::new(AtomicU32::new(0));
    spawn(client, &worker_id, &cancel, &counter);
    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let after_cancel = counter.load(Ordering::SeqCst);
    assert!(after_cancel > 0);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(counter.load(Ordering::SeqCst), after_cancel);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failed_publish_does_not_stop_the_loop() -> TestResult {
    let client = AdvancedMockNatsClient::new();
    client.fail_next_publish();
    let worker_id = WorkerId::new("hb-fail")?;
    let cancel = CancellationToken::new();
    let counter = Arc::new(AtomicU32::new(0));
    let (id, builds) = (worker_id.clone(), counter.clone());
    spawn_heartbeat_loop(
        client.clone(),
        ProviderKind::Mock,
        worker_id,
        Duration::from_millis(20),
        cancel.clone(),
        move || {
            let (id, builds) = (id.clone(), builds.clone());
            async move { heartbeat(&id, builds.fetch_add(1, Ordering::SeqCst)) }
        },
    );
    tokio::time::sleep(Duration::from_millis(70)).await;
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert!(counter.load(Ordering::SeqCst) >= 3);
    assert!(!client.published_messages().is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_unserializable_heartbeat_is_skipped() -> TestResult {
    let client = MockNatsClient::new();
    let worker_id = WorkerId::new("hb-bad")?;
    let cancel = CancellationToken::new();
    let counter = Arc::new(AtomicU32::new(0));
    let (id, builds) = (worker_id.clone(), counter.clone());
    spawn_heartbeat_loop(
        client.clone(),
        ProviderKind::Mock,
        worker_id,
        Duration::from_millis(20),
        cancel.clone(),
        move || {
            let (id, builds) = (id.clone(), builds.clone());
            async move {
                let mut beat = heartbeat(&id, builds.fetch_add(1, Ordering::SeqCst));
                beat.emitted_at = SystemTime::UNIX_EPOCH - Duration::from_secs(1);
                beat
            }
        },
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert!(counter.load(Ordering::SeqCst) >= 2);
    assert!(client.published_messages().is_empty());
    Ok(())
}
