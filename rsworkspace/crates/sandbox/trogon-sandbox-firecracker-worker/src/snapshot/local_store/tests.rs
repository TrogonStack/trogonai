type TestResult = Result<(), Box<dyn std::error::Error>>;
use std::num::NonZeroUsize;

use super::*;
use crate::config::SnapshotDir;

fn store(dir: &Path, keep: usize) -> anyhow::Result<SnapshotStore> {
    Ok(SnapshotStore::new(SnapshotConfig {
        dir: SnapshotDir::new(dir),
        keep: NonZeroUsize::new(keep).ok_or_else(|| anyhow::anyhow!("keep must be positive"))?,
    }))
}

async fn written(store: &SnapshotStore) -> anyhow::Result<Staged> {
    let staged = store.stage().await?;
    for file in staged.files().all() {
        tokio::fs::write(file, b"x").await?;
    }
    Ok(staged)
}

#[tokio::test]
async fn captured_snapshot_keeps_metadata_and_owns_private_copies() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let store = store(&tmp.path().join("cache"), 4)?;
    let snapshot = super::super::LocalSnapshot::fixture(tmp.path(), 1)?;
    let source = snapshot.path().to_owned();
    let expected = snapshot.metadata.clone();
    let stored = store.commit_captured(snapshot).await?;
    assert!(!source.exists());
    let held = store
        .hold(&stored.digest)
        .await
        .ok_or("the stored snapshot must be held")?;
    assert_eq!(std::fs::read(held.files().vm_state())?, b"state");
    assert_eq!(std::fs::read(held.files().rootfs())?, b"rootfs");
    let metadata: super::super::SnapshotMetadata = serde_json::from_slice(&std::fs::read(held.files().metadata())?)?;
    assert_eq!(metadata.memory_mib, expected.memory_mib);
    assert_eq!(metadata.rootfs_bytes, expected.rootfs_bytes);
    for file in held.files().all().into_iter().chain([held.files().metadata()]) {
        assert_eq!(std::fs::metadata(file)?.mode() & 0o777, 0o600);
    }
    Ok(())
}

#[tokio::test]
async fn local_restore_metadata_checks_config_and_preserves_legacy_entries() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let store = store(&tmp.path().join("cache"), 4)?;
    let snapshot = super::super::LocalSnapshot::fixture(tmp.path(), 1)?;
    let stored = store.commit_captured(snapshot).await?;
    let held = store
        .hold(&stored.digest)
        .await
        .ok_or("the stored snapshot must be held")?;
    let machine = super::super::RestoreMachine {
        memory: crate::config::MemoryMib::new(1)?,
        vcpus: crate::config::VcpuCount::new(2)?,
        rootfs_max_bytes: 1024,
    };
    assert!(held.files().restore_metadata(machine).await?.is_some());
    let wrong_vcpu = super::super::RestoreMachine {
        vcpus: crate::config::VcpuCount::new(1)?,
        ..machine
    };
    assert!(held.files().restore_metadata(wrong_vcpu).await.is_err());
    let file = std::fs::OpenOptions::new().write(true).open(held.files().metadata())?;
    file.set_len(super::super::MAX_METADATA_BYTES + 1)?;
    assert!(
        held.files()
            .restore_metadata(machine)
            .await
            .err()
            .ok_or("the call must fail")?
            .to_string()
            .contains("2 MiB")
    );
    drop(file);
    tokio::fs::remove_file(held.files().metadata()).await?;
    assert!(held.files().restore_metadata(machine).await?.is_none());
    let wrong_memory = super::super::RestoreMachine {
        memory: crate::config::MemoryMib::new(2)?,
        ..machine
    };
    assert!(held.files().restore_metadata(wrong_memory).await.is_err());
    let oversized_rootfs = super::super::RestoreMachine {
        rootfs_max_bytes: 5,
        ..machine
    };
    assert!(held.files().restore_metadata(oversized_rootfs).await.is_err());
    Ok(())
}

#[tokio::test]
async fn a_staged_snapshot_is_restorable_only_once_committed() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let store = store(tmp.path(), 4)?;
    let staged = written(&store).await?;
    let digest = digest_of(&staged.id).ok_or("a minted id is a digest")?;
    assert!(store.hold(&digest).await.is_none());
    assert!(store.digests().await.is_empty());

    let stored = store.commit(staged).await?;
    assert_eq!(stored.digest, digest);
    assert!(stored.bytes > 0);
    let held = store.hold(&digest).await.ok_or("the stored snapshot must be held")?;
    assert_eq!(
        held.files().memory(),
        tmp.path()
            .join(digest.as_str().trim_start_matches(DIGEST_PREFIX))
            .join(MEMORY)
    );
    drop(held);
    assert_eq!(store.digests().await, vec![digest]);
    Ok(())
}

#[tokio::test]
async fn only_the_newest_snapshots_are_kept() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let store = store(tmp.path(), 2)?;
    let mut digests = Vec::new();
    for _ in 0..3 {
        digests.push(store.commit(written(&store).await?).await?.digest);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(store.digests().await, vec![digests[2].clone(), digests[1].clone()]);
    assert!(store.hold(&digests[0]).await.is_none());
    Ok(())
}

#[tokio::test]
async fn unfinished_and_unknown_snapshots_are_never_restored() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let store = store(tmp.path(), 4)?;
    let staged = written(&store).await?;
    let digest = digest_of(&staged.id).ok_or("a minted id is a digest")?;
    store.discard_unfinished().await;
    assert!(!staged.files().0.exists());
    assert!(store.hold(&digest).await.is_none());

    let other = Digest::new(format!("{DIGEST_PREFIX}{}", "0".repeat(64)))?;
    assert!(store.hold(&other).await.is_none());
    Ok(())
}

#[test]
fn only_a_sha256_hex_id_names_a_snapshot_path() -> TestResult {
    let id = "0".repeat(64);
    assert_eq!(snapshot_id(&format!("{DIGEST_PREFIX}{id}")), Some(id.as_str()));
    for bad in [
        "sha256:../../outside".to_string(),
        format!("sha256:../{}", "0".repeat(61)),
        format!("sha256:{}", "A".repeat(64)),
        "0".repeat(64),
    ] {
        assert_eq!(snapshot_id(&bad), None, "{bad}");
    }
    Ok(())
}

#[tokio::test]
async fn a_discarded_snapshot_leaves_nothing_behind() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let store = store(tmp.path(), 4)?;
    let staged = written(&store).await?;
    store.discard(staged).await;
    assert_eq!(std::fs::read_dir(tmp.path())?.count(), 0);
    Ok(())
}
