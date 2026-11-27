use trogon_sandbox_contract::{CredentialError, CredentialId};

/// Outcome of [`CredentialBroker::migrate_legacy`](crate::CredentialBroker::migrate_legacy).
#[derive(Debug, Default)]
pub struct MigrationReport {
    /// Records rewritten in the envelope format.
    pub migrated: Vec<CredentialId>,
    /// Records a dry run would have rewritten. Empty on a real run.
    pub would_migrate: Vec<CredentialId>,
    pub failed: Vec<(CredentialId, CredentialError)>,
    /// Legacy records still present after the run. Zero means legacy reads can be turned off.
    pub remaining: usize,
}
