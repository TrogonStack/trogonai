use crate::active_lease::ActiveLeaseStore;
use crate::bus::BrokerBus;
use async_nats::jetstream;
use trogon_sandbox_contract::{SnapshotDeleted, SnapshotTagRecord};

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::{evict_snapshot, sweep_once};
use crate::quota::{QuotaStore, SnapshotQuota};
use crate::snapshot_tag::SnapshotTagStore;
use bytes::Bytes;
use trogon_sandbox_contract::{Digest, RetentionPolicy, TenantId};

use std::str::FromStr;
use std::sync::Mutex;

use async_trait::async_trait;
use trogon_sandbox_contract::{
    ArtifactType, EvictedArtifactAction, RetentionTagLimit, ServiceAccountId, SnapshotTag, SnapshotTagName,
};

use super::*;
use crate::snapshot_accounting::accepted::Accepted;
use crate::snapshot_tag::{InMemorySnapshotTagAuditor, InMemorySnapshotTagStore};

fn digest(hex: &str) -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::from_str(&format!("sha256:{hex:0<64}"))?)
}
fn days_ago(days: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(days * 24 * 60 * 60)
}

fn policy(
    max_tags: u32,
    max_age_days: u32,
    delete_evicted: bool,
) -> Result<RetentionPolicy, Box<dyn std::error::Error>> {
    Ok(RetentionPolicy {
        max_tags: RetentionTagLimit::new(max_tags)?,
        max_age_days,
        delete_evicted: EvictedArtifactAction::from(delete_evicted),
    })
}

#[test]
fn nothing_to_evict_when_history_small_and_recent() -> Result<(), Box<dyn std::error::Error>> {
    let current = digest("a")?;
    let history = vec![(digest("b")?, days_ago(1)), (digest("c")?, days_ago(2))];
    let out = select_eviction_candidates(&current, &history, &policy(5, 30, false)?, SystemTime::now());
    assert!(out.is_empty());
    Ok(())
}
#[test]
fn max_tags_evicts_oldest_excess() -> Result<(), Box<dyn std::error::Error>> {
    let current = digest("a")?;
    let history = vec![
        (digest("b")?, days_ago(1)),
        (digest("c")?, days_ago(2)),
        (digest("d")?, days_ago(3)),
    ];
    // max_tags = 2 means current + 1 historical entry survive by count.
    let out = select_eviction_candidates(&current, &history, &policy(2, 3650, false)?, SystemTime::now());
    assert_eq!(out, vec![digest("c")?, digest("d")?]);
    Ok(())
}
#[test]
fn max_age_evicts_entries_older_than_cutoff_even_within_count() -> Result<(), Box<dyn std::error::Error>> {
    let current = digest("a")?;
    let history = vec![(digest("b")?, days_ago(1)), (digest("c")?, days_ago(100))];
    let out = select_eviction_candidates(&current, &history, &policy(10, 30, false)?, SystemTime::now());
    assert_eq!(out, vec![digest("c")?]);
    Ok(())
}
#[test]
fn combined_count_and_age_conditions() -> Result<(), Box<dyn std::error::Error>> {
    let current = digest("a")?;
    let history = vec![
        (digest("b")?, days_ago(1)),
        (digest("c")?, days_ago(2)),
        (digest("d")?, days_ago(100)),
    ];
    // max_tags=2 -> only newest 1 historical entry survives by count (digest b).
    // max_age=30 -> digest d also fails on age.
    let out = select_eviction_candidates(&current, &history, &policy(2, 30, false)?, SystemTime::now());
    assert_eq!(out.len(), 2);
    assert!(out.contains(&digest("c")?));
    assert!(out.contains(&digest("d")?));
    Ok(())
}
#[test]
fn last_used_reset_keeps_a_frequently_leased_digest_out_of_eviction() -> Result<(), Box<dyn std::error::Error>> {
    let current = digest("a")?;
    let mut history = vec![(digest("b")?, days_ago(100))];
    let recently_used: HashMap<Digest, SystemTime> = [(digest("b")?, days_ago(1))].into_iter().collect();
    apply_last_used_resets(&mut history, &recently_used);

    let out = select_eviction_candidates(&current, &history, &policy(10, 30, false)?, SystemTime::now());
    assert!(out.is_empty(), "recently-used digest should not be evicted");
    Ok(())
}
#[test]
fn last_used_reset_ignores_a_last_used_time_older_than_the_audit_time() -> Result<(), Box<dyn std::error::Error>> {
    let audit_at = days_ago(1);
    let mut history = vec![(digest("b")?, audit_at)];
    let stale_last_used: HashMap<Digest, SystemTime> = [(digest("b")?, days_ago(100))].into_iter().collect();
    apply_last_used_resets(&mut history, &stale_last_used);

    assert_eq!(history, vec![(digest("b")?, audit_at)]);
    Ok(())
}
#[test]
fn last_used_reset_is_a_noop_for_digests_never_used_directly() -> Result<(), Box<dyn std::error::Error>> {
    let at = days_ago(5);
    let mut history = vec![(digest("b")?, at)];
    apply_last_used_resets(&mut history, &HashMap::new());
    assert_eq!(history, vec![(digest("b")?, at)]);
    Ok(())
}
#[test]
fn current_digest_never_appears_even_if_present_in_history_and_old() -> Result<(), Box<dyn std::error::Error>> {
    let current = digest("a")?;
    let history = vec![(current.clone(), days_ago(9999))];
    let out = select_eviction_candidates(&current, &history, &policy(1, 1, false)?, SystemTime::now());
    assert!(out.is_empty());
    assert!(!out.contains(&current));
    Ok(())
}
#[tokio::test]
async fn the_tag_store_enumerates_tenants_from_its_keys() -> Result<(), Box<dyn std::error::Error>> {
    let store = in_memory_store();
    for (tenant, name) in [("beta", "prod"), ("alpha", "a"), ("alpha", "b"), ("a.b", "x")] {
        let tag = SnapshotTag {
            tenant: TenantId::new(tenant)?,
            name: SnapshotTagName::new(name)?,
        };
        store.set(&tag, test_record(digest("1")?, None), None).await?;
    }
    let tenants: Vec<String> = store
        .tenants()
        .await?
        .into_iter()
        .map(|t| t.as_str().to_owned())
        .collect();
    assert_eq!(tenants, vec!["a.b", "alpha", "beta"]);
    Ok(())
}
#[tokio::test]
async fn discovery_adds_store_tenants_to_the_configured_list_once() -> Result<(), Box<dyn std::error::Error>> {
    let store = in_memory_store();
    let tag = SnapshotTag {
        tenant: TenantId::new("alpha")?,
        name: SnapshotTagName::new("a")?,
    };
    store.set(&tag, test_record(digest("1")?, None), None).await?;
    let store: Arc<dyn SnapshotTagStore> = Arc::new(store);
    let mut config = RetentionSweepConfig {
        tenants: vec![TenantId::new("alpha")?, TenantId::new("gamma")?],
        ..Default::default()
    };
    let names = |v: Vec<TenantId>| v.into_iter().map(|t| t.as_str().to_owned()).collect::<Vec<_>>();
    assert_eq!(names(resolve_tenants(&config, &store).await), vec!["alpha", "gamma"]);
    config.discover_tenants = true;
    config.tenants = vec![TenantId::new("gamma")?];
    assert_eq!(names(resolve_tenants(&config, &store).await), vec!["gamma", "alpha"]);
    Ok(())
}
#[tokio::test]
async fn live_restore_sources_come_from_the_active_lease_store() -> Result<(), Box<dyn std::error::Error>> {
    let leases = crate::active_lease::InMemoryActiveLeaseStore::new();
    let mut live = crate::idle::tests::record(&[])?;
    live.restored_from = Some(digest("a")?);
    leases.put(&live).await?;
    let config = RetentionSweepConfig {
        active_leases: Some(Arc::new(leases)),
        ..Default::default()
    };
    let sources = live_restore_sources(&config).await.ok_or("restore sources")?;
    assert!(sources.contains(&digest("a")?));
    assert_eq!(
        live_restore_sources(&RetentionSweepConfig::default()).await,
        Some(HashSet::new())
    );
    Ok(())
}
struct CaptureBus {
    published: Arc<Mutex<Vec<(String, Bytes)>>>,
}

impl CaptureBus {
    #[allow(clippy::type_complexity)]
    fn new() -> (Self, Arc<Mutex<Vec<(String, Bytes)>>>) {
        let published = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                published: Arc::clone(&published),
            },
            published,
        )
    }
}

#[async_trait]
impl BrokerBus for CaptureBus {
    async fn request(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<Bytes> {
        Ok(Bytes::new())
    }

    async fn publish(&self, subject: &str, payload: Bytes) -> anyhow::Result<()> {
        self.published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((subject.to_string(), payload));
        Ok(())
    }
}

fn in_memory_store() -> InMemorySnapshotTagStore {
    InMemorySnapshotTagStore::new(Arc::new(InMemorySnapshotTagAuditor::new()))
}

fn test_record(digest: Digest, retention: Option<RetentionPolicy>) -> SnapshotTagRecord {
    SnapshotTagRecord {
        digest,
        artifact_type: ArtifactType::HyperlightSnapshotV1,
        set_by: ServiceAccountId::new("test-operator"),
        set_at: SystemTime::now(),
        retention,
    }
}

#[tokio::test]
async fn eviction_candidates_never_include_current_tag_digest() -> Result<(), Box<dyn std::error::Error>> {
    let store = in_memory_store();
    let tenant = TenantId::new("acme")?;
    let tag = SnapshotTag {
        tenant: tenant.clone(),
        name: SnapshotTagName::new("prod")?,
    };

    let current = digest("a")?;
    store
        .set(&tag, test_record(current.clone(), Some(policy(1, 1, false)?)), None)
        .await?;

    let (bus, published) = CaptureBus::new();
    let bus: Arc<dyn BrokerBus> = Arc::new(bus);

    let record = store.get(&tag).await?.ok_or("not found")?;
    let history = vec![(current.clone(), days_ago(9999))];
    let candidates = select_eviction_candidates(
        &record.digest,
        &history,
        record.retention.as_ref().ok_or("no retention")?,
        SystemTime::now(),
    );
    assert!(candidates.is_empty());

    for candidate in candidates {
        evict_snapshot(
            &candidate,
            &tenant,
            &record,
            &RetentionSweepConfig {
                tenants: vec![tenant.clone()],
                registry: "registry.example.com".to_string(),
                ..Default::default()
            },
            &Arc::new(trogon_sandbox_oci::OciClient::new(
                trogon_sandbox_oci::OciClientConfig::default(),
            )),
            &bus,
        )
        .await;
    }

    assert!(
        published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    Ok(())
}
struct UnexpectedRemoval(Arc<std::sync::atomic::AtomicUsize>);

#[async_trait::async_trait]
impl crate::snapshot_tracker::SnapshotRemover for UnexpectedRemoval {
    async fn remove(
        &self,
        _: &TenantId,
        _: &Digest,
    ) -> Result<crate::snapshot_tracker::SnapshotRemoval, crate::snapshot_tracker::SnapshotRemovalError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(crate::snapshot_tracker::SnapshotRemovalError::ClaimRequired)
    }
}

#[tokio::test]
async fn retained_registry_artifact_does_not_publish_a_deletion_or_refund() -> Result<(), Box<dyn std::error::Error>> {
    let tenant = TenantId::new("acme")?;
    let record = test_record(digest("a")?, Some(policy(1, 1, false)?));
    let (bus, published) = CaptureBus::new();
    let bus: Arc<dyn BrokerBus> = Arc::new(bus);
    let oci_client = Arc::new(trogon_sandbox_oci::OciClient::new(
        trogon_sandbox_oci::OciClientConfig::default(),
    ));
    let quota = crate::quota::InMemoryQuotaStore::new();
    let accounting = quota.snapshot_store().ok_or("snapshot store")?;
    let live = crate::snapshot_tracker::InMemoryLiveSnapshotStore::new();
    let evicted = digest("b")?;
    let reservation = accounting
        .reserve(
            &tenant,
            &trogon_sandbox_contract::LeaseId::new("retained-artifact"),
            "retained-artifact",
        )
        .await
        .accepted()?;
    accounting
        .bind(
            &reservation,
            trogon_sandbox_contract::OciArtifactRef {
                registry: "registry.example.com".into(),
                repository: trogon_sandbox_protocol::snapshot_repository_for_tenant(&tenant),
                digest: evicted.clone(),
                artifact_type: record.artifact_type,
            },
            &trogon_sandbox_contract::SnapshotReceipt {
                digest: evicted.clone(),
                artifact_type: record.artifact_type,
                bytes: 500,
            },
        )
        .await
        .accepted()?;
    accounting.seal(&reservation).await.accepted()?;
    let publication = accounting.publication(&reservation).await.accepted()?;
    crate::snapshot_tracker::handle_snapshot_published(live.as_ref(), &quota, SnapshotQuota::default(), publication)
        .await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let config = RetentionSweepConfig {
        tenants: vec![tenant.clone()],
        registry: "registry.example.com".to_string(),
        remover: Some(Arc::new(UnexpectedRemoval(calls.clone()))),
        ..Default::default()
    };

    evict_snapshot(&evicted, &tenant, &record, &config, &oci_client, &bus).await;

    assert!(
        published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "delete_evicted=false preserves the artifact and its accounting obligation"
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    let retained = quota.quota_record(&tenant).await?.ok_or("not found")?;
    assert_eq!((retained.snapshot.count_used, retained.snapshot.bytes_used), (1, 500));
    assert!(
        crate::snapshot_tracker::LiveSnapshotStore::get_owned(live.as_ref(), &tenant, &evicted)
            .await?
            .is_some()
    );
    Ok(())
}
async fn make_nats_tag_store(
    client: async_nats::Client,
    bucket_suffix: &str,
) -> Result<(Arc<dyn SnapshotTagStore>, jetstream::Context), Box<dyn std::error::Error>> {
    let js = jetstream::new(client);
    let audit = Arc::new(crate::snapshot_tag::JetStreamSnapshotTagAuditor::new(js.clone(), 1).await?);
    let kv = js
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: format!(
                "{}-{bucket_suffix}",
                trogon_sandbox_protocol::subjects::SNAPSHOT_TAG_KV_BUCKET
            ),
            history: 64,
            ..Default::default()
        })
        .await?;
    let store: Arc<dyn SnapshotTagStore> = Arc::new(crate::snapshot_tag::NatsKvSnapshotTagStore { kv, audit });
    Ok((store, js))
}

#[tokio::test]
#[ignore = "requires nats-server with JetStream on PATH"]
async fn sweep_never_evicts_a_digest_currently_pinned_by_another_named_tag() -> Result<(), Box<dyn std::error::Error>> {
    let server = crate::test_support::LocalNats::start().await;
    let suffix = crate::generated_ulid::generated_ulid();
    let (tag_store, js) = make_nats_tag_store(server.client.clone(), &suffix).await?;
    let tenant = TenantId::new("pinned-tag-tenant")?;

    let prod_tag = SnapshotTag {
        tenant: tenant.clone(),
        name: SnapshotTagName::new("prod")?,
    };
    let auto_tag = SnapshotTag {
        tenant: tenant.clone(),
        name: SnapshotTagName::new("auto")?,
    };

    let pinned_digest = digest("abc1")?;
    tag_store
        .set(&prod_tag, test_record(pinned_digest.clone(), None), None)
        .await?;

    tag_store
        .set(&auto_tag, test_record(pinned_digest.clone(), None), None)
        .await?;
    for i in 0..4u32 {
        let record = test_record(digest(&format!("f{i}"))?, Some(policy(1, 3650, false)?));
        tag_store.set(&auto_tag, record, None).await?;
    }

    let last_used_store: Arc<dyn crate::snapshot_last_used::SnapshotLastUsedStore> =
        Arc::new(crate::snapshot_last_used::InMemorySnapshotLastUsedStore::new());
    let oci_client = Arc::new(trogon_sandbox_oci::OciClient::new(
        trogon_sandbox_oci::OciClientConfig::default(),
    ));
    let (bus, published) = CaptureBus::new();
    let bus: Arc<dyn BrokerBus> = Arc::new(bus);
    let config = RetentionSweepConfig {
        tenants: vec![tenant.clone()],
        registry: "registry.example.com".to_string(),
        ..Default::default()
    };

    sweep_once(&config, &tag_store, &last_used_store, &js, &oci_client, &bus).await;

    let evicted_digests: Vec<Digest> = published
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .map(|(_, payload)| serde_json::from_slice::<SnapshotDeleted>(payload).map(|event| event.digest))
        .collect::<Result<_, serde_json::Error>>()?;
    assert!(
        !evicted_digests.contains(&pinned_digest),
        "digest pinned by the prod tag must never be reported as evicted by the sweep, got {evicted_digests:?}"
    );

    let prod_record = tag_store.get(&prod_tag).await?.ok_or("not found")?;
    assert_eq!(
        prod_record.digest, pinned_digest,
        "prod tag itself must be untouched by the sweep"
    );
    Ok(())
}
