/// A cold lease Job still serving its lease, as found in the cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredLeaseJob {
    pub lease_id: String,
    pub tenant_id: Option<String>,
    pub job_name: String,
    /// The Job's pod, which runs its `agent` container.
    pub pod_name: String,
}
