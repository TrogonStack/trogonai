use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use trogon_sandbox_contract::{LeaseId, TenantId};

use crate::active_lease::ActiveLeaseStore;
use crate::auth::AuthContext;
use crate::core::BrokerCore;
use crate::quota::{QuotaStore, SnapshotQuota};
use crate::snapshot_tracker::LiveSnapshotStore;

/// Everything a route needs to answer one message.
pub(crate) struct RouteContext {
    pub(crate) nats: async_nats::Client,
    pub(crate) broker: Arc<BrokerCore>,
    pub(crate) auth: AuthContext,
    /// Tenant of every lease this broker granted, read by the digest pin watcher.
    pub(crate) lease_tenants: Arc<RwLock<HashMap<LeaseId, TenantId>>>,
    pub(crate) live_snapshots: Arc<dyn LiveSnapshotStore>,
    pub(crate) quota: Arc<dyn QuotaStore>,
    pub(crate) default_snapshot_quota: SnapshotQuota,
    pub(crate) active_leases: Arc<dyn ActiveLeaseStore>,
}
