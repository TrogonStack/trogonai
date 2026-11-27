use trogon_sandbox_contract::SnapshotGeneration;

/// Outcome of removing a snapshot's registry artifact on a tenant's behalf.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SnapshotRemoval {
    Removed {
        bytes: u64,
        generation: Option<SnapshotGeneration>,
    },
    NotFound,
}
