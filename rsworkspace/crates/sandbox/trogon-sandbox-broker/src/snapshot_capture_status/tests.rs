use std::str::FromStr;
use trogon_sandbox_contract::{ArtifactType, Digest, LeaseId, SnapshotCaptureStatus};

use super::*;

fn lease(id: &str) -> LeaseId {
    LeaseId::new(id)
}

#[tokio::test]
async fn in_memory_roundtrip_created() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemorySnapshotCaptureStatusStore::new();
    let status = SnapshotCaptureStatus::Created {
        digest: Digest::from_str("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")?,
        artifact_type: ArtifactType::HyperlightSnapshotV1,
    };
    store.record(&lease("lease-1"), status.clone()).await?;
    assert_eq!(store.get(&lease("lease-1")).await?, Some(status));
    Ok(())
}
#[tokio::test]
async fn in_memory_roundtrip_failed() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemorySnapshotCaptureStatusStore::new();
    let status = SnapshotCaptureStatus::Failed {
        reason: "export failed".to_string(),
    };
    store.record(&lease("lease-2"), status.clone()).await?;
    assert_eq!(store.get(&lease("lease-2")).await?, Some(status));
    Ok(())
}
#[tokio::test]
async fn in_memory_overwrites_on_re_record() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemorySnapshotCaptureStatusStore::new();
    store
        .record(
            &lease("lease-3"),
            SnapshotCaptureStatus::Failed {
                reason: "first attempt failed".to_string(),
            },
        )
        .await?;
    let created = SnapshotCaptureStatus::Created {
        digest: Digest::from_str("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")?,
        artifact_type: ArtifactType::HyperlightSnapshotV1,
    };
    store.record(&lease("lease-3"), created.clone()).await?;
    assert_eq!(store.get(&lease("lease-3")).await?, Some(created));
    Ok(())
}
#[tokio::test]
async fn in_memory_records_are_independent_per_lease() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemorySnapshotCaptureStatusStore::new();
    store
        .record(
            &lease("lease-a"),
            SnapshotCaptureStatus::Failed {
                reason: "a failed".to_string(),
            },
        )
        .await?;
    assert_eq!(store.get(&lease("lease-b")).await?, None);
    Ok(())
}
#[tokio::test]
async fn noop_store_never_persists() -> Result<(), Box<dyn std::error::Error>> {
    let store = NoopSnapshotCaptureStatusStore;
    store
        .record(
            &lease("lease-1"),
            SnapshotCaptureStatus::Failed {
                reason: "ignored".to_string(),
            },
        )
        .await?;
    assert_eq!(store.get(&lease("lease-1")).await?, None);
    Ok(())
}
