use crate::k8s_ops::RecoveredLeaseJob;

/// What a sweep of the namespace's lease Jobs found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LeaseJobSweep {
    /// Jobs still serving a lease.
    pub live: Vec<RecoveredLeaseJob>,
    /// Jobs torn down because their lease could no longer run.
    pub swept: Vec<String>,
}
