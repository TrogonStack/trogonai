/// What [`crate::K8sWorkerProvider::recover_leases`] found.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LeaseRecovery {
    /// Cold lease Jobs taken back into the lease table.
    pub cold: usize,
    /// Claimed warm pods taken back into the lease table.
    pub warm: usize,
    /// Lease Jobs torn down because their lease could no longer run.
    pub swept: usize,
}
