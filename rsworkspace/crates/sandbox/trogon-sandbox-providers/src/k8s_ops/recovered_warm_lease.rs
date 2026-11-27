/// A warm pod claimed for a lease, as found in the cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredWarmLease {
    pub lease_id: String,
    pub pod_name: String,
    pub restore_secret_name: Option<String>,
}
