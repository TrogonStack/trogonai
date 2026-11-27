use std::time::{Duration, SystemTime};

use tracing::{info, warn};
use trogon_sandbox_broker::snapshot_tag::{SnapshotTagStore, SnapshotTagStoreError};
use trogon_sandbox_contract::{
    LeaseId, ServiceAccountId, SnapshotPublished, SnapshotTag, SnapshotTagName, SnapshotTagRecord,
};

use crate::constants::{MAX_TAG_CAS_ATTEMPTS, TAG_SET_BY};
use crate::release_tag::ReleaseTag;

/// Moves the tag, then moves it again on a failure after each delay in `backoff`, then gives up. The move is
/// idempotent, so a retry never rebuilds or republishes the snapshot.
pub(crate) async fn auto_tag_with_retry(
    tag_store: &dyn SnapshotTagStore,
    release: &ReleaseTag<'_>,
    observed_at: SystemTime,
    backoff: &[Duration],
) -> Result<(), SnapshotTagStoreError> {
    let mut attempt = 0;
    loop {
        match auto_tag_snapshot(tag_store, release, observed_at).await {
            Ok(()) => return Ok(()),
            Err(error) => match backoff.get(attempt) {
                Some(delay) => {
                    warn!(
                        lease_id = %release.event.lease_id.as_str(),
                        error = %error,
                        attempt = attempt + 1,
                        "retrying snapshot tag move"
                    );
                    tokio::time::sleep(*delay).await;
                    attempt += 1;
                }
                None => return Err(error),
            },
        }
    }
}

/// Broker release time prevents a slower, older capture from moving the tag backwards. Arrival time is the
/// compatibility fallback for older brokers.
pub(crate) async fn auto_tag_snapshot(
    tag_store: &dyn SnapshotTagStore,
    release: &ReleaseTag<'_>,
    observed_at: SystemTime,
) -> Result<(), SnapshotTagStoreError> {
    let event = release.event;
    let observed_at = event.released_at.unwrap_or(observed_at);
    let name = match SnapshotTagName::new(release.tag_name) {
        Ok(name) => name,
        Err(error) => {
            warn!(
                lease_id = %event.lease_id.as_str(),
                tag_name = %release.tag_name,
                error = %error,
                correlation_id = %release.correlation_id(),
                "skipping snapshot auto-tag: invalid tag name"
            );
            return Ok(());
        }
    };
    let tag = SnapshotTag {
        tenant: event.tenant.clone(),
        name,
    };
    let record = SnapshotTagRecord {
        digest: release.receipt.digest.clone(),
        artifact_type: release.receipt.artifact_type,
        set_by: ServiceAccountId::new(TAG_SET_BY),
        set_at: observed_at,
        retention: release.retention,
    };

    auto_tag_record(tag_store, &event.lease_id, &tag, record, release.correlation_id()).await
}

/// Moves the tag a published snapshot carries, if it carries one.
pub(crate) async fn auto_tag_published(
    tag_store: &dyn SnapshotTagStore,
    published: &SnapshotPublished,
) -> Result<(), SnapshotTagStoreError> {
    let Some(intent) = published.tag.as_ref() else {
        return Ok(());
    };
    let tag = SnapshotTag {
        tenant: published.tenant.clone(),
        name: intent.name.clone(),
    };
    let record = SnapshotTagRecord {
        digest: published.digest.clone(),
        artifact_type: published.artifact_type,
        set_by: ServiceAccountId::new(TAG_SET_BY),
        set_at: intent.set_at,
        retention: intent.retention,
    };
    let correlation_id = published.correlation_id.as_ref().map_or("unknown", |id| id.as_str());
    auto_tag_record(tag_store, &published.lease_id, &tag, record, correlation_id).await
}

async fn auto_tag_record(
    tag_store: &dyn SnapshotTagStore,
    lease_id: &LeaseId,
    tag: &SnapshotTag,
    record: SnapshotTagRecord,
    correlation_id: &str,
) -> Result<(), SnapshotTagStoreError> {
    let observed_at = record.set_at;

    // Compare-and-swap against the revision we read so a concurrent writer is never overwritten blindly. The tag
    // is moved only after SnapshotPublished is out, so a stale tag is the only failure mode and a retry converges.
    let mut last_err = SnapshotTagStoreError::RevisionConflict;
    for _ in 0..MAX_TAG_CAS_ATTEMPTS {
        let expected = match tag_store.get_entry(tag).await {
            Ok(Some((current, _))) if current == record => return Ok(()),
            Ok(Some((current, _))) if current.set_at > observed_at => {
                info!(
                    lease_id = %lease_id.as_str(),
                    tag_name = %tag.name.as_str(),
                    current_digest = %current.digest,
                    correlation_id,
                    "skipping snapshot auto-tag: tag already points at a newer snapshot"
                );
                return Ok(());
            }
            Ok(Some((_, revision))) => Some(revision),
            Ok(None) => Some(0),
            Err(error) => {
                last_err = error;
                break;
            }
        };
        match tag_store.set(tag, record.clone(), expected).await {
            Ok(_) => return Ok(()),
            Err(SnapshotTagStoreError::RevisionConflict) => {
                last_err = SnapshotTagStoreError::RevisionConflict;
            }
            Err(error) => {
                last_err = error;
                break;
            }
        }
    }
    warn!(
        lease_id = %lease_id.as_str(),
        tag_name = %tag.name.as_str(),
        error = %last_err,
        correlation_id,
        "failed to auto-tag snapshot"
    );
    Err(last_err)
}

#[cfg(test)]
mod tests;
