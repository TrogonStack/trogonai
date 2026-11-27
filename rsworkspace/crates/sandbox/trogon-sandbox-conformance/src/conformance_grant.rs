use trogon_sandbox_contract::{LeaseId, ProviderKind, ProviderLease};

use crate::NativeCleanupAuthority;

/// A grant a worker returned during a check, with the authority it carries for cleanup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConformanceGrant {
    pub provider: ProviderKind,
    pub lease_id: LeaseId,
    pub native: ProviderLease,
    pub native_cleanup_authority: NativeCleanupAuthority,
}
