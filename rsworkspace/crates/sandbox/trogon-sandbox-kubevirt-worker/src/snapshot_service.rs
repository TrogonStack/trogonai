use std::sync::Arc;

use crate::{RestoreStores, SnapshotRegistry};

/// Restores snapshots into DataVolumes and exports VM disks as snapshots, through an OCI registry and an S3
/// staging store.
pub struct SnapshotService {
    pub(crate) registry: Arc<dyn SnapshotRegistry>,
    pub(crate) stores: Arc<dyn RestoreStores>,
}

impl SnapshotService {
    pub fn new(registry: Arc<dyn SnapshotRegistry>, stores: Arc<dyn RestoreStores>) -> Self {
        Self { registry, stores }
    }
}
