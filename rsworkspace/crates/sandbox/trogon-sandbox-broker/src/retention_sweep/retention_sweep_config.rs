use std::sync::Arc;

use trogon_sandbox_contract::TenantId;

use crate::active_lease::ActiveLeaseStore;
use crate::snapshot_catalog::CatalogSweeper;

#[derive(Default)]
pub struct RetentionSweepConfig {
    pub tenants: Vec<TenantId>,
    pub registry: String,
    /// Also sweep every tenant found in the tag store's keys.
    pub discover_tenants: bool,
    /// Source of the leases whose restore sources are protected. Without it
    /// only tag pins protect a digest, as before.
    pub active_leases: Option<Arc<dyn ActiveLeaseStore>>,
    /// Keep-N and orphan pass, run after the tag pass.
    pub catalog: Option<Arc<CatalogSweeper>>,
    pub remover: Option<Arc<dyn crate::snapshot_tracker::SnapshotRemover>>,
}
