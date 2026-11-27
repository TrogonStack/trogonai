use std::str::FromStr;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::bus::BrokerBus;
use async_trait::async_trait;
use bytes::Bytes;
use trogon_sandbox_contract::{Digest, LeaseRecord, LeaseStatus, OciArtifactRef, SnapshotKvRecord, TenantId};
use trogon_sandbox_protocol::subjects::SNAPSHOT_DELETED;

use std::sync::Mutex as StdMutex;

use trogon_sandbox_contract::{ArtifactType, LeaseId};

use super::*;
use crate::snapshot_accounting::accepted::Accepted;

fn digest(hex: &str) -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::from_str(&format!("sha256:{hex:0<64}"))?)
}
fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

fn record(
    tenant: &str,
    lease: &str,
    hex: &str,
    created: Option<u64>,
    parent: Option<&str>,
) -> Result<SnapshotKvRecord, Box<dyn std::error::Error>> {
    Ok(SnapshotKvRecord {
        lease_id: LeaseId::new(lease),
        tenant: TenantId::new(tenant)?,
        digest: digest(hex)?,
        artifact_type: ArtifactType::HyperlightSnapshotV1,
        registry: "registry.example.com".into(),
        repository: trogon_sandbox_protocol::snapshot_repository_for_tenant(&TenantId::new(tenant)?),
        parent: parent.map(digest).transpose()?,
        created_at: created.map(at),
        env_digest: None,
    })
}
fn digests(records: &[SnapshotKvRecord]) -> Vec<Digest> {
    records.iter().map(|r| r.digest.clone()).collect()
}

#[test]
fn shared_digest_keeps_every_tenants_recorded_parent_chain_protected() -> Result<(), Box<dyn std::error::Error>> {
    let records = vec![
        record("first", "l1", "a", Some(3), Some("b"))?,
        record("second", "l2", "a", Some(3), Some("c"))?,
        record("first", "l1", "b", Some(2), Some("d"))?,
        record("second", "l2", "c", Some(2), Some("e"))?,
    ];
    let protected = close_over_parents([digest("a")?].into_iter().collect(), &records);
    for hex in ["a", "b", "c", "d", "e"] {
        assert!(
            protected.contains(&digest(hex)?),
            "a shared content digest must retain the parent edge for every owner: {hex}"
        );
    }
    Ok(())
}
#[test]
fn keep_n_prunes_the_oldest_beyond_the_budget_per_lease() -> Result<(), Box<dyn std::error::Error>> {
    let records = vec![
        record("t", "l1", "a", Some(10), None)?,
        record("t", "l1", "b", Some(20), None)?,
        record("t", "l1", "c", Some(30), None)?,
        record("t", "l2", "d", Some(5), None)?,
    ];
    let out = plan_keep_n(&records, 2, &HashSet::new());
    assert_eq!(digests(&out), vec![digest("a")?]);
    Ok(())
}
#[test]
fn keep_n_never_prunes_a_protected_digest() -> Result<(), Box<dyn std::error::Error>> {
    let records = vec![
        record("t", "l1", "a", Some(10), None)?,
        record("t", "l1", "b", Some(20), None)?,
        record("t", "l1", "c", Some(30), None)?,
    ];
    let protected: HashSet<Digest> = [digest("a")?].into_iter().collect();
    let digest_a = digest("a")?;
    assert!(
        plan_keep_n(&records, 1, &protected)
            .iter()
            .all(|r| r.digest != digest_a)
    );
    assert_eq!(digests(&plan_keep_n(&records, 1, &protected)), vec![digest("b")?]);
    Ok(())
}
#[test]
fn keep_n_zero_disables_and_unknown_age_is_never_pruned() -> Result<(), Box<dyn std::error::Error>> {
    let records = vec![
        record("t", "l1", "a", None, None)?,
        record("t", "l1", "b", Some(20), None)?,
    ];
    assert!(plan_keep_n(&records, 0, &HashSet::new()).is_empty());
    assert!(plan_keep_n(&records, 1, &HashSet::new()).is_empty());
    Ok(())
}
#[test]
fn tenants_do_not_share_a_budget() -> Result<(), Box<dyn std::error::Error>> {
    let records = vec![
        record("t1", "l", "a", Some(1), None)?,
        record("t2", "l", "b", Some(2), None)?,
    ];
    assert!(plan_keep_n(&records, 1, &HashSet::new()).is_empty());
    Ok(())
}
#[test]
fn protection_follows_the_parent_chain() -> Result<(), Box<dyn std::error::Error>> {
    let records = vec![
        record("t", "l", "a", Some(1), None)?,
        record("t", "l", "b", Some(2), Some("a"))?,
        record("t", "l", "c", Some(3), Some("b"))?,
    ];
    let protected = close_over_parents([digest("c")?].into_iter().collect(), &records);
    assert_eq!(protected.len(), 3);
    assert!(plan_keep_n(&records, 1, &protected).is_empty());
    Ok(())
}
#[test]
fn a_parent_cycle_terminates() -> Result<(), Box<dyn std::error::Error>> {
    let records = vec![
        record("t", "l", "a", Some(1), Some("b"))?,
        record("t", "l", "b", Some(2), Some("a"))?,
    ];
    let out = close_over_parents([digest("a")?].into_iter().collect(), &records);
    assert_eq!(out.len(), 2);
    Ok(())
}
fn lease(
    status: LeaseStatus,
    name: Option<&str>,
    from: Option<&str>,
) -> Result<LeaseRecord, Box<dyn std::error::Error>> {
    let mut r = crate::idle::tests::record(&[])?;
    r.status = status;
    r.name = name.map(trogon_sandbox_contract::LeaseName::new).transpose()?;
    r.restored_from = from.map(digest).transpose()?;
    Ok(r)
}

#[test]
fn live_and_named_leases_protect_their_restore_source() -> Result<(), Box<dyn std::error::Error>> {
    let leases = vec![
        lease(LeaseStatus::Running, None, Some("a"))?,
        lease(LeaseStatus::Suspended, None, Some("b"))?,
        lease(LeaseStatus::Released, Some("keepme"), Some("c"))?,
        lease(LeaseStatus::Released, None, Some("d"))?,
        lease(LeaseStatus::Failed, None, Some("e"))?,
    ];
    let out = lease_restore_sources(&leases);
    assert!(out.contains(&digest("a")?));
    assert!(out.contains(&digest("b")?));
    assert!(out.contains(&digest("c")?));
    assert!(!out.contains(&digest("d")?));
    assert!(!out.contains(&digest("e")?));
    Ok(())
}
fn artifact(tenant: &str, hex: &str) -> Result<ListedArtifact, Box<dyn std::error::Error>> {
    Ok(ListedArtifact {
        tenant: TenantId::new(tenant)?,
        reference: OciArtifactRef {
            registry: "registry.example.com".into(),
            repository: trogon_sandbox_protocol::snapshot_repository_for_tenant(&TenantId::new(tenant)?),
            digest: digest(hex)?,
            artifact_type: ArtifactType::HyperlightSnapshotV1,
        },
    })
}
#[test]
fn orphans_exclude_cataloged_and_protected_artifacts() -> Result<(), Box<dyn std::error::Error>> {
    let catalog = vec![record("t", "l", "a", Some(1), None)?];
    let protected: HashSet<Digest> = [digest("e")?].into_iter().collect();
    let listed = vec![artifact("t", "a")?, artifact("t", "e")?, artifact("t", "f")?];
    assert_eq!(find_orphans(&listed, &catalog, &protected), vec![artifact("t", "f")?]);
    Ok(())
}
#[test]
fn an_orphan_is_ready_only_after_the_grace_has_passed() -> Result<(), Box<dyn std::error::Error>> {
    let mut tracker = OrphanTracker::default();
    let grace = Duration::from_secs(600);
    let o = vec![artifact("t", "f")?];
    assert!(tracker.observe(&o, at(1000), grace).is_empty());
    assert!(tracker.observe(&o, at(1500), grace).is_empty());
    assert_eq!(tracker.observe(&o, at(1600), grace), o);
    Ok(())
}

#[test]
fn an_artifact_that_gains_a_record_resets_its_clock() -> Result<(), Box<dyn std::error::Error>> {
    let mut tracker = OrphanTracker::default();
    let grace = Duration::from_secs(600);
    let o = vec![artifact("t", "f")?];
    tracker.observe(&o, at(1000), grace);
    tracker.observe(&[], at(1300), grace);
    assert!(tracker.observe(&o, at(1700), grace).is_empty());
    Ok(())
}

#[test]
fn zero_grace_still_needs_a_second_sighting() -> Result<(), Box<dyn std::error::Error>> {
    let mut tracker = OrphanTracker::default();
    let o = vec![artifact("t", "f")?];
    assert!(tracker.observe(&o, at(1000), Duration::ZERO).is_empty());
    assert_eq!(tracker.observe(&o, at(1001), Duration::ZERO), o);
    Ok(())
}

struct FakeDeleter {
    deleted: StdMutex<Vec<Digest>>,
    fail: bool,
}

#[async_trait]
impl ArtifactDeleter for FakeDeleter {
    async fn delete(&self, artifact: &OciArtifactRef) -> Result<(), ArtifactDeleteError> {
        if self.fail {
            return Err(trogon_sandbox_oci::OciError::RegistryRejected("registry down".to_owned()).into());
        }
        self.deleted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(artifact.digest.clone());
        Ok(())
    }
}

struct FakeLister(Vec<ListedArtifact>);

#[async_trait]
impl ArtifactLister for FakeLister {
    async fn list(&self) -> Result<Vec<ListedArtifact>, ArtifactListError> {
        Ok(self.0.clone())
    }
}

struct CaptureBus(StdMutex<Vec<String>>);

#[async_trait]
impl BrokerBus for CaptureBus {
    async fn request(&self, _s: &str, _p: Bytes) -> anyhow::Result<Bytes> {
        Ok(Bytes::new())
    }
    async fn publish(&self, subject: &str, _p: Bytes) -> anyhow::Result<()> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(subject.to_string());
        Ok(())
    }
}

async fn sweeper(
    records: Vec<SnapshotKvRecord>,
    lister: Option<Vec<ListedArtifact>>,
    fail: bool,
) -> Result<
    (
        CatalogSweeper,
        Arc<InMemorySnapshotCatalog>,
        Arc<FakeDeleter>,
        Arc<CaptureBus>,
    ),
    Box<dyn std::error::Error>,
> {
    let accounting = crate::snapshot_accounting::SnapshotAccountingStore::memory();
    let live = crate::snapshot_live::InMemoryLiveSnapshotStore::new();
    let mut captures: Vec<_> = records
        .iter()
        .map(|record| {
            (
                record.tenant.clone(),
                record.lease_id.clone(),
                OciArtifactRef {
                    registry: record.registry.clone(),
                    repository: record.repository.clone(),
                    digest: record.digest.clone(),
                    artifact_type: record.artifact_type,
                },
            )
        })
        .collect();
    captures.extend(lister.iter().flatten().map(|artifact| {
        (
            artifact.tenant.clone(),
            LeaseId::new("orphan-fixture"),
            artifact.reference.clone(),
        )
    }));
    for (tenant, lease, reference) in captures {
        let reservation = accounting
            .reserve(&tenant, &lease, &format!("{}:{}", lease.as_str(), reference.digest))
            .await
            .accepted()?;
        accounting
            .bind(
                &reservation,
                reference.clone(),
                &trogon_sandbox_contract::SnapshotReceipt {
                    digest: reference.digest,
                    artifact_type: reference.artifact_type,
                    bytes: 100,
                },
            )
            .await
            .accepted()?;
        accounting.seal(&reservation).await.accepted()?;
        let event = accounting.publication(&reservation).await.accepted()?;
        accounting
            .apply_publication(&event, crate::quota::SnapshotQuota::default(), None)
            .await
            .accepted()?;
    }
    let catalog = Arc::new(InMemorySnapshotCatalog::default());
    for r in records {
        catalog.insert(r).await;
    }
    let deleter = Arc::new(FakeDeleter {
        deleted: StdMutex::new(Vec::new()),
        fail,
    });
    let bus = Arc::new(CaptureBus(StdMutex::new(Vec::new())));
    let s = CatalogSweeper::new(
        catalog.clone(),
        lister.map(|l| Arc::new(FakeLister(l)) as Arc<dyn ArtifactLister>),
        deleter.clone(),
        bus.clone(),
        CatalogSweepConfig {
            keep_per_lease: 1,
            orphan_grace: Duration::from_secs(60),
        },
    )
    .with_accounting(accounting, live);
    Ok((s, catalog, deleter, bus))
}
#[tokio::test]
async fn sweep_prunes_artifact_record_and_announces_it() -> Result<(), Box<dyn std::error::Error>> {
    let (s, catalog, deleter, bus) = sweeper(
        vec![
            record("t", "l", "a", Some(1), None)?,
            record("t", "l", "b", Some(2), None)?,
        ],
        None,
        false,
    )
    .await?;
    let report = s.sweep_once(HashSet::new(), HashSet::new(), at(100)).await;
    assert_eq!(report.pruned, 1);
    assert_eq!(
        *deleter
            .deleted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![digest("a")?]
    );
    assert_eq!(digests(&catalog.all().await?), vec![digest("b")?]);
    assert_eq!(
        *bus.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![SNAPSHOT_DELETED.to_string()]
    );
    Ok(())
}
#[tokio::test]
async fn a_failed_registry_delete_keeps_the_record_for_the_next_pass() -> Result<(), Box<dyn std::error::Error>> {
    let (s, catalog, _, bus) = sweeper(
        vec![
            record("t", "l", "a", Some(1), None)?,
            record("t", "l", "b", Some(2), None)?,
        ],
        None,
        true,
    )
    .await?;
    let report = s.sweep_once(HashSet::new(), HashSet::new(), at(100)).await;
    assert_eq!(report, CatalogSweepReport::default());
    assert_eq!(catalog.all().await?.len(), 2);
    assert!(
        bus.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    Ok(())
}
#[tokio::test]
async fn a_restore_source_survives_even_when_it_is_over_budget() -> Result<(), Box<dyn std::error::Error>> {
    let (s, _, deleter, _) = sweeper(
        vec![
            record("t", "l", "a", Some(1), None)?,
            record("t", "l", "b", Some(2), None)?,
        ],
        None,
        false,
    )
    .await?;
    let sources: HashSet<Digest> = [digest("a")?].into_iter().collect();
    let report = s.sweep_once(sources, HashSet::new(), at(100)).await;
    assert_eq!(report.pruned, 0);
    assert!(
        deleter
            .deleted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    Ok(())
}
#[tokio::test]
async fn orphans_are_deleted_on_the_second_sweep_after_the_grace() -> Result<(), Box<dyn std::error::Error>> {
    let (s, _, deleter, _) = sweeper(
        vec![record("t", "l", "a", Some(1), None)?],
        Some(vec![artifact("t", "a")?, artifact("t", "f")?]),
        false,
    )
    .await?;
    let first = s.sweep_once(HashSet::new(), HashSet::new(), at(100)).await;
    assert_eq!(first.orphans_deleted, 0);
    let second = s.sweep_once(HashSet::new(), HashSet::new(), at(200)).await;
    assert_eq!(second.orphans_deleted, 1);
    assert_eq!(
        *deleter
            .deleted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![digest("f")?]
    );
    Ok(())
}
