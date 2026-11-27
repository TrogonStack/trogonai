use super::*;

#[async_trait]
pub trait CommandLaunchStore: Send + Sync {
    fn cancellation_custody_support(&self) -> CancellationCustodySupport {
        CancellationCustodySupport::Unsupported
    }

    /// Atomically claims `key` for `lease` as pending with `fingerprint`, or
    /// returns what an earlier call recorded for it.
    async fn claim(&self, lease: &LeaseId, key: &LaunchKey, fingerprint: &str) -> Result<Claim, CommandLaunchError>;

    async fn retain_cancellation_authority(
        &self,
        _lease: &LeaseId,
        _key: &LaunchKey,
        _authority: &CommandCancellationAuthority,
    ) -> Result<CancellationAuthorityClaim, CommandLaunchError> {
        Err(CommandLaunchError::CustodyUnsupported)
    }

    async fn cancellation_authority(
        &self,
        _lease: &LeaseId,
        _key: &LaunchKey,
    ) -> Result<Option<CommandCancellationAuthority>, CommandLaunchError> {
        Err(CommandLaunchError::CustodyUnsupported)
    }

    /// Records the worker's reply for a claimed key.
    async fn finish(&self, lease: &LeaseId, key: &LaunchKey, launch: &CommandLaunch) -> Result<(), CommandLaunchError>;

    /// Drops a claimed key, so a retry dispatches again. Used when the launch
    /// never reached a worker.
    async fn forget(&self, lease: &LeaseId, key: &LaunchKey) -> Result<(), CommandLaunchError>;

    /// Replaces the entry for `key` with `next` only while it still equals
    /// `expected`. Returns false when another call changed or removed it.
    async fn replace(
        &self,
        lease: &LeaseId,
        key: &LaunchKey,
        expected: &CommandLaunch,
        next: &CommandLaunch,
    ) -> Result<bool, CommandLaunchError>;

    /// Drops every key of an ended lease.
    async fn forget_lease(&self, lease: &LeaseId) -> Result<(), CommandLaunchError>;
}
