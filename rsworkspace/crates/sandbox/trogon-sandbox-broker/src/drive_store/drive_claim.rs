use trogon_sandbox_contract::{DriveId, LeaseId};

/// A drive's claim by a lease, as the orphan sweep sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveClaim {
    pub drive_id: DriveId,
    pub lease_id: LeaseId,
    /// Store revision the claim was read at; a release guarded by it cannot
    /// remove a claim written after the read.
    pub revision: u64,
    /// When the store wrote this revision, when it records that.
    pub claimed_at: Option<std::time::SystemTime>,
}
