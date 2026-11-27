type TestResult = Result<(), Box<dyn std::error::Error>>;
use super::*;
use trogon_sandbox_contract::{ArtifactType, Digest};

#[test]
fn an_inherited_descriptor_does_not_outlive_the_depot_owner() -> TestResult {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join(DIRECTORY);
    let owner = SnapshotDepot::open(&root, &worker()?, Default::default())?;
    let inherited = owner._lock.try_clone()?;
    drop(owner);
    let replacement = SnapshotDepot::open(&root, &worker()?, Default::default());
    drop(inherited);
    assert!(replacement.is_ok(), "the depot guard must own its lock lifetime");
    Ok(())
}

fn policy() -> SnapshotPolicy {
    SnapshotPolicy {
        artifact_type: ArtifactType::FirecrackerSnapshotV1,
        target_repository: "snapshots/test".into(),
        retention: None,
        prewarm: None,
        tag_name: None,
    }
}
fn worker() -> anyhow::Result<WorkerId> {
    Ok(WorkerId::new("worker-a")?)
}
fn deadline() -> SystemTime {
    SystemTime::now() + Duration::from_secs(60)
}
fn target() -> SnapshotPushTarget {
    SnapshotPushTarget {
        registry: "registry.example".into(),
        repository: policy().target_repository,
        artifact_type: ArtifactType::FirecrackerSnapshotV1,
    }
}
fn receipt() -> anyhow::Result<SnapshotReceipt> {
    Ok(SnapshotReceipt {
        digest: Digest::new(format!("sha256:{}", "a".repeat(64)))?,
        bytes: 100,
        artifact_type: ArtifactType::FirecrackerSnapshotV1,
    })
}

#[tokio::test]
async fn byte_reservations_admit_only_snapshots_that_fit_and_shrink_to_actual_size() -> TestResult {
    let parent = tempfile::tempdir()?;
    let depot = SnapshotDepot::open(
        &parent.path().join(DIRECTORY),
        &worker()?,
        RetentionConfig {
            max_bytes: 3 * 1024 * 1024,
            ..Default::default()
        },
    )?;
    let first = LeaseId::new("first");
    let second = LeaseId::new("second");
    depot.arm(&first, &policy(), 2 * 1024 * 1024, deadline())?;
    assert!(depot.arm(&second, &policy(), 2 * 1024 * 1024, deadline()).is_err());
    depot.retain(&first, LocalSnapshot::fixture(parent.path(), 1)?).await?;
    depot.arm(&second, &policy(), 1536 * 1024, deadline())?;
    Ok(())
}

#[test]
fn a_no_ttl_lease_arms_for_a_century() -> TestResult {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join(DIRECTORY);
    let lease = LeaseId::new("no-ttl");
    let depot = SnapshotDepot::open(&root, &worker()?, Default::default())?;
    let far = SystemTime::now() + Duration::from_secs(100 * 365 * 24 * 60 * 60);
    depot.arm(&lease, &policy(), 2 * 1024 * 1024, far)?;
    let entry = depot.get(&lease)?.ok_or("the armed entry must exist")?;
    assert!(entry.expires_at >= far);
    assert!(matches!(entry.state, RetainedState::Armed { .. }));
    Ok(())
}

#[tokio::test]
async fn committed_files_recover_after_a_crash_before_database_phase_update() -> TestResult {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join(DIRECTORY);
    let lease = LeaseId::new("lease");
    let depot = SnapshotDepot::open(&root, &worker()?, Default::default())?;
    depot.arm(&lease, &policy(), 2 * 1024 * 1024, deadline())?;
    LocalSnapshot::fixture(parent.path(), 1)?
        .seal_to(&depot.directory(&lease))
        .await?;
    drop(depot);
    let recovered = SnapshotDepot::open(&root, &worker()?, Default::default())?;
    recovered.recover().await?;
    assert!(matches!(
        recovered.get(&lease)?.ok_or("the recovered entry must exist")?.state,
        RetainedState::Ready { .. }
    ));
    assert!(directory_info(&recovered.directory(&lease)).await?.bytes > 1024 * 1024);
    Ok(())
}

#[tokio::test]
async fn exact_receipts_survive_restart_without_retaining_snapshot_files_and_purge_discards_them() -> TestResult {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join(DIRECTORY);
    let lease = LeaseId::new("lease");
    let depot = SnapshotDepot::open(&root, &worker()?, Default::default())?;
    depot.arm(&lease, &policy(), 2 * 1024 * 1024, deadline())?;
    depot.retain(&lease, LocalSnapshot::fixture(parent.path(), 1)?).await?;
    depot.claim_target(&lease, &target())?;
    depot.record_receipt(&lease, &target(), &receipt()?)?;
    drop(depot);
    let recovered = SnapshotDepot::open(&root, &worker()?, Default::default())?;
    recovered.recover().await?;
    assert!(!recovered.directory(&lease).exists());
    assert_eq!(recovered.receipt(&lease, &target())?, Some(receipt()?));
    let mut wrong = target();
    wrong.registry = "other.example".into();
    assert!(recovered.receipt(&lease, &wrong).is_err());
    recovered.mark_purged(&lease).await?;
    assert!(recovered.receipt(&lease, &target()).is_err());
    recovered.delete_purged(&lease)?;
    assert!(recovered.get(&lease)?.is_none());
    Ok(())
}

#[tokio::test]
async fn expired_entries_can_be_removed_without_time_based_waits() -> TestResult {
    let parent = tempfile::tempdir()?;
    let lease = LeaseId::new("lease");
    let depot = SnapshotDepot::open(
        &parent.path().join(DIRECTORY),
        &worker()?,
        RetentionConfig {
            export_ttl: Duration::from_secs(10),
            ..Default::default()
        },
    )?;
    depot.arm(&lease, &policy(), 2 * 1024 * 1024, deadline())?;
    depot.retain(&lease, LocalSnapshot::fixture(parent.path(), 1)?).await?;
    assert_eq!(
        depot.expired(SystemTime::now() + Duration::from_secs(11))?.as_slice(),
        std::slice::from_ref(&lease)
    );
    depot.mark_purged(&lease).await?;
    depot.delete_purged(&lease)?;
    assert!(!depot.directory(&lease).exists());
    assert!(depot.get(&lease)?.is_none());
    Ok(())
}
