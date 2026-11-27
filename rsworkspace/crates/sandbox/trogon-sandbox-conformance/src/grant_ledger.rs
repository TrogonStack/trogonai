use std::sync::{Arc, Mutex};

use trogon_sandbox_contract::{LeaseId, ProviderKind, ProviderLease};

use crate::{ConformanceGrant, NativeCleanupAuthority};

/// Records every grant the suite received, so a caller can clean up what a failed check left behind.
#[derive(Debug, Clone, Default)]
pub struct GrantLedger {
    grants: Arc<Mutex<Vec<ConformanceGrant>>>,
}

impl GrantLedger {
    /// The grants recorded so far, in arrival order.
    pub fn snapshot(&self) -> Vec<ConformanceGrant> {
        self.grants
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(crate) fn record(
        &self,
        provider: ProviderKind,
        lease_id: &LeaseId,
        native: &ProviderLease,
        native_cleanup_authority: NativeCleanupAuthority,
    ) {
        self.grants
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(ConformanceGrant {
                provider,
                lease_id: lease_id.clone(),
                native: native.clone(),
                native_cleanup_authority,
            });
    }
}

#[cfg(test)]
mod tests;
