mod support;

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use async_nats::jetstream::kv;
use bytes::Bytes;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    CapabilitySupport, Digest, ProviderKind, TenantId, WorkerCapacity, WorkerDispatchProtocol, WorkerHeartbeat,
    WorkerId,
};
use trogon_sandbox_protocol::subjects::{
    SNAPSHOT_KV_BUCKET, SNAPSHOT_LIVE_KV_BUCKET, SNAPSHOT_PUBLISHED, SNAPSHOT_TAG_KV_BUCKET, snapshot_kv_key,
    worker_heartbeat,
};
use trogon_sandbox_protocol::{legacy_tenant_scoped_kv_prefix, tenant_scoped_kv_key};
use trogon_sandbox_ws_gateway::{CapabilityRegistry, NatsObservability, ObservabilityConfig, ObservabilityError};

use support::{TestResult, mock_profile};

const LEASES: &str = "observability-leases";
const WAIT: Duration = Duration::from_secs(10);

fn time(secs: u64) -> Value {
    json!({"secs_since_epoch": secs, "nanos_since_epoch": 0})
}

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn lease(id: &str, tenant: &str, tags: Value) -> Value {
    json!({
        "lease_id": id,
        "tenant": tenant,
        "provider": "mock",
        "status": "running",
        "created_at": time(100),
        "deadline": time(700),
        "tags": tags,
        "spec": {
            "image": {"kind": "registry", "reference": {"registry": "ghcr.io", "repository": "acme/app"}},
            "workspace": {"empty": {}},
            "cpu_seconds": 10,
            "memory_mib": 512,
            "env": {},
            "privileged": false,
            "boot_mode": "entrypoint_default",
        },
    })
}

fn snapshot(tenant: &str, digest: &str) -> Value {
    json!({
        "lease_id": "lease-s",
        "tenant": tenant,
        "digest": digest,
        "artifact_type": "sandbox_rootfs_v1",
        "registry": "ghcr.io",
        "repository": "acme/snapshots",
    })
}

fn live(tenant: &str) -> Value {
    json!({"tenant": tenant, "artifact_type": "sandbox_rootfs_v1", "published_at": time(300), "bytes": 4096})
}

fn tag(digest: &str, owner: Option<(&str, &str)>) -> Value {
    let mut value = json!({
        "digest": digest,
        "artifact_type": "sandbox_rootfs_v1",
        "set_by": "sa-1",
        "set_at": time(500),
    });
    if let Some((tenant, name)) = owner {
        value["owner"] = json!({"tenant": tenant, "name": name});
    }
    value
}

fn heartbeat(provider: ProviderKind, worker: &str, emitted_at: SystemTime) -> TestResult<Vec<u8>> {
    let beat = WorkerHeartbeat {
        supports_lease_epoch: CapabilitySupport::Unsupported,
        worker_id: WorkerId::new(worker)?,
        provider,
        capacity: WorkerCapacity {
            max_concurrent_leases: 4,
            current_leases: 1,
            capabilities: Default::default(),
        },
        emitted_at,
        cached_digests: vec![Digest::new(digest('d'))?],
        profile: mock_profile(CapabilitySupport::Supported),
        warm_pool_population: Default::default(),
        dispatch_protocol: WorkerDispatchProtocol::TargetedV1,
        native_volume_bindings: Default::default(),
    };
    Ok(serde_json::to_vec(&beat)?)
}

async fn bucket(server: &JetStreamTestServer, name: &str) -> TestResult<kv::Store> {
    Ok(server
        .jetstream()
        .await
        .create_key_value(kv::Config {
            bucket: name.to_string(),
            ..Default::default()
        })
        .await?)
}

async fn put(store: &kv::Store, key: &str, value: &Value) -> TestResult {
    store.put(key, Bytes::from(serde_json::to_vec(value)?)).await?;
    Ok(())
}

async fn observability(server: &JetStreamTestServer) -> NatsObservability {
    NatsObservability::new(
        server.client().await,
        ObservabilityConfig {
            active_lease_bucket: LEASES.to_string(),
        },
    )
}

async fn next(rx: &mut mpsc::Receiver<(String, Value)>) -> TestResult<(String, Value)> {
    Ok(tokio::time::timeout(WAIT, rx.recv())
        .await?
        .ok_or("the watcher stopped")?)
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(300)).await;
}

#[tokio::test]
async fn listings_over_missing_buckets_fail_with_the_bucket_name() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let observability = observability(&server).await;
    let tenant = TenantId::new("acme")?;
    let leases = observability.list_leases(&tenant, &BTreeMap::new()).await;
    assert!(matches!(leases, Err(ObservabilityError::Bucket { ref bucket, .. }) if bucket == LEASES));
    assert!(matches!(
        observability.list_snapshots(&tenant).await,
        Err(ObservabilityError::Bucket { .. })
    ));
    assert!(matches!(
        observability.list_tags(&tenant).await,
        Err(ObservabilityError::Bucket { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn the_default_config_names_the_active_lease_bucket() -> TestResult {
    let server = JetStreamTestServer::start().await;
    bucket(&server, "sandbox-active-leases").await?;
    let observability = NatsObservability::new(server.client().await, ObservabilityConfig::default());
    let listing = observability
        .list_leases(&TenantId::new("acme")?, &BTreeMap::new())
        .await?;
    assert_eq!(listing, json!({"leases": []}));
    Ok(())
}

#[tokio::test]
async fn leases_are_listed_for_the_tenant_sorted_and_filtered_by_tags() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let store = bucket(&server, LEASES).await?;
    put(&store, "lease-b", &lease("lease-b", "acme", json!({"team": "red"}))).await?;
    put(&store, "lease-a", &lease("lease-a", "acme", json!({"team": "blue"}))).await?;
    put(&store, "lease-c", &lease("lease-c", "other", json!({}))).await?;
    store.put("lease-gone", Bytes::from_static(b"{}")).await?;
    store.delete("lease-gone").await?;
    let observability = observability(&server).await;
    let tenant = TenantId::new("acme")?;

    let all = observability.list_leases(&tenant, &BTreeMap::new()).await?;
    let ids: Vec<&str> = all["leases"]
        .as_array()
        .ok_or("no leases")?
        .iter()
        .filter_map(|row| row["lease_id"].as_str())
        .collect();
    assert_eq!(ids, vec!["lease-a", "lease-b"]);
    assert_eq!(all["leases"][0]["ttl_seconds"], 600);
    assert_eq!(all["leases"][0]["started_at"], 100);

    let filter = BTreeMap::from([("team".to_string(), "red".to_string())]);
    let red = observability.list_leases(&tenant, &filter).await?;
    assert_eq!(red["leases"].as_array().ok_or("no leases")?.len(), 1);

    store.put("lease-bad", Bytes::from_static(b"not json")).await?;
    assert!(matches!(
        observability.list_leases(&tenant, &BTreeMap::new()).await,
        Err(ObservabilityError::Decode(_))
    ));
    Ok(())
}

#[tokio::test]
async fn snapshots_prefer_the_tenant_scoped_key_and_carry_their_live_entry() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let records = bucket(&server, SNAPSHOT_KV_BUCKET).await?;
    let lives = bucket(&server, SNAPSHOT_LIVE_KV_BUCKET).await?;
    let tenant = TenantId::new("acme")?;
    let (d1, d2, d3, d4, d5) = (digest('1'), digest('2'), digest('3'), digest('4'), digest('5'));
    let scoped = |value: &str| tenant_scoped_kv_key(&tenant, value);

    put(&records, &scoped(&d1), &snapshot("acme", &d1)).await?;
    put(&records, &snapshot_kv_key(&Digest::new(&d2)?), &snapshot("acme", &d2)).await?;
    put(&records, &scoped(&d3), &snapshot("acme", &d3)).await?;
    put(&records, &snapshot_kv_key(&Digest::new(&d3)?), &snapshot("acme", &d3)).await?;
    put(&records, &scoped(&d4), &snapshot("other", &d4)).await?;
    put(&records, "unrelated-key", &snapshot("acme", &d5)).await?;
    records.put(scoped(&digest('6')), Bytes::from_static(b"x")).await?;
    records.delete(scoped(&digest('6'))).await?;
    put(&lives, &scoped(&d1), &live("acme")).await?;
    put(&lives, &scoped(&d2), &json!({"deleted": true})).await?;
    put(&lives, &scoped(&d3), &live("other")).await?;
    lives.put(scoped(&d4), Bytes::from_static(b"not json")).await?;

    let listing = observability(&server).await.list_snapshots(&tenant).await?;
    let rows = listing["snapshots"].as_array().ok_or("no snapshots")?;
    let digests: Vec<&str> = rows.iter().filter_map(|row| row["digest"].as_str()).collect();
    assert_eq!(digests, vec![d1.as_str(), d2.as_str(), d3.as_str()]);
    assert_eq!(rows[0]["bytes"], 4096);
    assert_eq!(rows[0]["published_at"], 300);
    assert_eq!(rows[1]["bytes"], Value::Null);
    assert_eq!(rows[2]["bytes"], Value::Null);
    Ok(())
}

#[tokio::test]
async fn a_live_bucket_that_is_missing_leaves_the_snapshot_without_live_data() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let records = bucket(&server, SNAPSHOT_KV_BUCKET).await?;
    let tenant = TenantId::new("acme")?;
    let d1 = digest('1');
    put(&records, &tenant_scoped_kv_key(&tenant, &d1), &snapshot("acme", &d1)).await?;
    let listing = observability(&server).await.list_snapshots(&tenant).await?;
    assert_eq!(listing["snapshots"][0]["bytes"], Value::Null);
    Ok(())
}

#[tokio::test]
async fn tags_resolve_by_owner_stamp_or_the_legacy_key() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let tags = bucket(&server, SNAPSHOT_TAG_KV_BUCKET).await?;
    let tenant = TenantId::new("acme")?;
    let legacy = legacy_tenant_scoped_kv_prefix(&tenant);
    let d1 = digest('1');

    put(
        &tags,
        &tenant_scoped_kv_key(&tenant, "stable"),
        &tag(&d1, Some(("acme", "stable"))),
    )
    .await?;
    put(&tags, &format!("{legacy}old"), &tag(&d1, None)).await?;
    put(&tags, &format!("{legacy}stamped"), &tag(&d1, Some(("acme", "stamped")))).await?;
    put(
        &tags,
        &tenant_scoped_kv_key(&tenant, "forged"),
        &tag(&d1, Some(("other", "forged"))),
    )
    .await?;
    put(
        &tags,
        &tenant_scoped_kv_key(&TenantId::new("other")?, "foreign"),
        &tag(&d1, Some(("other", "foreign"))),
    )
    .await?;
    tags.put(tenant_scoped_kv_key(&tenant, "removed"), Bytes::from_static(b"x"))
        .await?;
    tags.delete(tenant_scoped_kv_key(&tenant, "removed")).await?;

    let listing = observability(&server).await.list_tags(&tenant).await?;
    let names: Vec<&str> = listing["tags"]
        .as_array()
        .ok_or("no tags")?
        .iter()
        .filter_map(|row| row["name"].as_str())
        .collect();
    assert_eq!(names, vec!["old", "stable", "stamped"]);
    assert_eq!(listing["tags"][0]["set_at"], 500);
    Ok(())
}

#[tokio::test]
async fn the_lease_watcher_reports_tenant_changes_and_survives_bad_records() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let store = bucket(&server, LEASES).await?;
    let observability = observability(&server).await;
    let (tx, mut rx) = mpsc::channel(16);
    let cancel = CancellationToken::new();
    observability.spawn_lease_watcher(TenantId::new("acme")?, tx, cancel.clone());
    settle().await;

    put(&store, "lease-1", &lease("lease-1", "acme", json!({}))).await?;
    put(&store, "lease-2", &lease("lease-2", "other", json!({}))).await?;
    store.put("lease-3", Bytes::from_static(b"not json")).await?;
    let (stream, event) = next(&mut rx).await?;
    assert_eq!(stream, "sandbox.leases");
    assert_eq!(event["action"], "upsert");
    assert_eq!(event["lease"]["lease_id"], "lease-1");
    store.delete("lease-1").await?;
    put(&store, "lease-4", &lease("lease-4", "acme", json!({}))).await?;
    assert_eq!(next(&mut rx).await?.1["lease"]["lease_id"], "lease-4");
    cancel.cancel();
    Ok(())
}

#[tokio::test]
async fn the_lease_watcher_stops_when_its_receiver_is_gone_or_the_bucket_is_missing() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let store = bucket(&server, LEASES).await?;
    let observability = observability(&server).await;
    let (tx, rx) = mpsc::channel(1);
    drop(rx);
    observability.spawn_lease_watcher(TenantId::new("acme")?, tx, CancellationToken::new());
    settle().await;
    put(&store, "lease-1", &lease("lease-1", "acme", json!({}))).await?;
    settle().await;

    let empty = JetStreamTestServer::start().await;
    let (tx, mut rx) = mpsc::channel(1);
    self::observability(&empty)
        .await
        .spawn_lease_watcher(TenantId::new("acme")?, tx, CancellationToken::new());
    assert!(tokio::time::timeout(WAIT, rx.recv()).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn the_snapshot_watcher_reports_only_the_tenants_publications() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let observability = observability(&server).await;
    let client = server.client().await;
    let (tx, mut rx) = mpsc::channel(16);
    let cancel = CancellationToken::new();
    observability.spawn_snapshot_published_watcher(TenantId::new("acme")?, tx, cancel.clone());
    settle().await;

    let published = |tenant: &str| {
        json!({
            "lease_id": "lease-1",
            "digest": digest('1'),
            "artifact_type": "sandbox_rootfs_v1",
            "tenant": tenant,
            "bytes": 10,
        })
    };
    client
        .publish(SNAPSHOT_PUBLISHED, Bytes::from_static(b"not json"))
        .await?;
    client
        .publish(
            SNAPSHOT_PUBLISHED,
            Bytes::from(serde_json::to_vec(&published("other"))?),
        )
        .await?;
    client
        .publish(SNAPSHOT_PUBLISHED, Bytes::from(serde_json::to_vec(&published("acme"))?))
        .await?;
    let (stream, event) = next(&mut rx).await?;
    assert_eq!(stream, "sandbox.snapshots");
    assert_eq!(event["digest"], digest('1'));
    cancel.cancel();
    settle().await;

    let (tx, rx) = mpsc::channel(1);
    drop(rx);
    observability.spawn_snapshot_published_watcher(TenantId::new("acme")?, tx, CancellationToken::new());
    settle().await;
    client
        .publish(SNAPSHOT_PUBLISHED, Bytes::from(serde_json::to_vec(&published("acme"))?))
        .await?;
    settle().await;
    Ok(())
}

#[tokio::test]
async fn the_worker_watcher_reports_heartbeats_and_stops_with_its_receiver() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let observability = observability(&server).await;
    let client = server.client().await;
    let (tx, mut rx) = mpsc::channel(16);
    let cancel = CancellationToken::new();
    observability.spawn_worker_heartbeat_watcher(tx, cancel.clone());
    settle().await;

    let subject = worker_heartbeat(ProviderKind::Mock, &WorkerId::new("worker-1")?);
    client.publish(subject.clone(), Bytes::from_static(b"not json")).await?;
    client
        .publish(
            subject.clone(),
            Bytes::from(heartbeat(ProviderKind::Mock, "worker-1", SystemTime::now())?),
        )
        .await?;
    let (stream, event) = next(&mut rx).await?;
    assert_eq!(stream, "sandbox.workers");
    assert_eq!(event["worker_id"], "worker-1");
    assert_eq!(event["current_leases"], 1);
    assert_eq!(event["max_concurrent_leases"], 4);
    assert_eq!(event["cached_digests_count"], 1);
    cancel.cancel();
    settle().await;

    let (tx, rx) = mpsc::channel(1);
    drop(rx);
    observability.spawn_worker_heartbeat_watcher(tx, CancellationToken::new());
    settle().await;
    client
        .publish(
            subject,
            Bytes::from(heartbeat(ProviderKind::Mock, "worker-1", SystemTime::now())?),
        )
        .await?;
    settle().await;
    Ok(())
}

#[tokio::test]
async fn watchers_on_a_drained_client_stop_at_once() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    client.drain().await?;
    settle().await;
    let observability = NatsObservability::new(client.clone(), ObservabilityConfig::default());
    let (tx, mut rx) = mpsc::channel(1);
    observability.spawn_snapshot_published_watcher(TenantId::new("acme")?, tx.clone(), CancellationToken::new());
    observability.spawn_worker_heartbeat_watcher(tx, CancellationToken::new());
    CapabilityRegistry::spawn_heartbeat_watcher(client, CapabilityRegistry::new(), CancellationToken::new());
    assert!(tokio::time::timeout(WAIT, rx.recv()).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn the_registry_counts_fresh_heartbeats_and_ignores_stale_and_malformed_ones() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let registry = CapabilityRegistry::new();
    let cancel = CancellationToken::new();
    CapabilityRegistry::spawn_heartbeat_watcher(client.clone(), registry.clone(), cancel.clone());
    settle().await;

    let live_workers = |provider: ProviderKind| {
        let registry = registry.clone();
        async move {
            let snapshot = registry.snapshot().await;
            snapshot
                .providers
                .iter()
                .find(|entry| entry.kind == provider)
                .map_or(0, |entry| entry.live_workers)
        }
    };
    assert_eq!(live_workers(ProviderKind::Mock).await, 0);
    let mock_subject = worker_heartbeat(ProviderKind::Mock, &WorkerId::new("worker-1")?);
    client.publish(mock_subject, Bytes::from_static(b"not json")).await?;
    client
        .publish(
            worker_heartbeat(ProviderKind::Mock, &WorkerId::new("worker-1")?),
            Bytes::from(heartbeat(ProviderKind::Mock, "worker-1", SystemTime::now())?),
        )
        .await?;
    client
        .publish(
            worker_heartbeat(ProviderKind::E2b, &WorkerId::new("worker-2")?),
            Bytes::from(heartbeat(
                ProviderKind::E2b,
                "worker-2",
                SystemTime::now() - Duration::from_secs(3600),
            )?),
        )
        .await?;
    let deadline = tokio::time::Instant::now() + WAIT;
    while live_workers(ProviderKind::Mock).await == 0 {
        assert!(tokio::time::Instant::now() < deadline, "no heartbeat was recorded");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    settle().await;
    assert_eq!(live_workers(ProviderKind::E2b).await, 0);
    cancel.cancel();
    Ok(())
}
