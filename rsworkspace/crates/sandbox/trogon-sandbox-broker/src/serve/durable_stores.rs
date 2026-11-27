use std::sync::Arc;

use async_nats::jetstream;

use super::serve_error::ServeError;
use crate::core::{
    JetStreamSnapshotTagHistoryReader, NatsKvSnapshotKvReader, SnapshotKvReader, SnapshotTagHistoryReader,
};
use crate::drive_store::{DriveStore, NatsKvDriveStore};
use crate::named_lease::{NamedLeaseStore, NatsKvNamedLeaseStore};
use crate::native_snapshot::{NativeSnapshotStore, NatsKvNativeSnapshotStore};
use crate::pending_purge::{NatsKvPendingPurgeStore, PendingPurgeStore};
use crate::pending_release::{NatsKvPendingReleaseStore, PendingReleaseStore};
use crate::resolved_digest::{NatsKvResolvedDigestStore, ResolvedDigestStore};
use crate::snapshot_last_used::{NatsKvSnapshotLastUsedStore, SnapshotLastUsedStore};
use crate::snapshot_tag::{JetStreamSnapshotTagAuditor, NatsKvSnapshotTagStore, SnapshotTagStore};
use crate::usage_store::{LeaseUsageStore, NatsKvLeaseUsageStore};

/// The JetStream backed stores that always live in NATS, whatever the quota backend.
pub(super) struct DurableStores {
    pub(super) tags: Arc<dyn SnapshotTagStore>,
    pub(super) resolved_digests: Arc<dyn ResolvedDigestStore>,
    pub(super) usage: Arc<dyn LeaseUsageStore>,
    pub(super) snapshot_last_used: Arc<dyn SnapshotLastUsedStore>,
    pub(super) native_snapshots: Arc<dyn NativeSnapshotStore>,
    pub(super) drives: Arc<dyn DriveStore>,
    pub(super) named_leases: Arc<dyn NamedLeaseStore>,
    pub(super) pending_purges: Arc<dyn PendingPurgeStore>,
    pub(super) pending_releases: Arc<dyn PendingReleaseStore>,
    pub(super) snapshot_kv_reader: Arc<dyn SnapshotKvReader>,
    pub(super) tag_history_reader: Arc<dyn SnapshotTagHistoryReader>,
}

impl DurableStores {
    pub(super) async fn open(jetstream: &jetstream::Context, replicas: usize) -> Result<Self, ServeError> {
        let tag_audit = Arc::new(
            JetStreamSnapshotTagAuditor::new(jetstream.clone(), replicas)
                .await
                .map_err(ServeError::opening("snapshot tag audit stream"))?,
        );
        Ok(Self {
            tags: Arc::new(
                NatsKvSnapshotTagStore::new(jetstream.clone(), tag_audit)
                    .await
                    .map_err(ServeError::opening("snapshot tag store"))?,
            ),
            resolved_digests: Arc::new(
                NatsKvResolvedDigestStore::new(jetstream.clone())
                    .await
                    .map_err(ServeError::opening("resolved digest store"))?,
            ),
            usage: Arc::new(
                NatsKvLeaseUsageStore::new(jetstream.clone())
                    .await
                    .map_err(ServeError::opening("lease usage store"))?,
            ),
            snapshot_last_used: Arc::new(
                NatsKvSnapshotLastUsedStore::new(jetstream.clone())
                    .await
                    .map_err(ServeError::opening("snapshot last-used store"))?,
            ),
            native_snapshots: Arc::new(
                NatsKvNativeSnapshotStore::new(jetstream.clone())
                    .await
                    .map_err(ServeError::opening("native snapshot store"))?,
            ),
            drives: Arc::new(
                NatsKvDriveStore::new(jetstream.clone())
                    .await
                    .map_err(ServeError::opening("drive store"))?,
            ),
            named_leases: Arc::new(
                NatsKvNamedLeaseStore::new(jetstream.clone())
                    .await
                    .map_err(ServeError::opening("named lease store"))?,
            ),
            pending_purges: Arc::new(
                NatsKvPendingPurgeStore::new(jetstream.clone())
                    .await
                    .map_err(ServeError::opening("pending purge store"))?,
            ),
            pending_releases: Arc::new(
                NatsKvPendingReleaseStore::new(jetstream.clone())
                    .await
                    .map_err(ServeError::opening("pending release store"))?,
            ),
            snapshot_kv_reader: Arc::new(
                NatsKvSnapshotKvReader::new(jetstream.clone())
                    .await
                    .map_err(ServeError::opening("snapshot reader"))?,
            ),
            tag_history_reader: Arc::new(JetStreamSnapshotTagHistoryReader::new(jetstream.clone())),
        })
    }
}
