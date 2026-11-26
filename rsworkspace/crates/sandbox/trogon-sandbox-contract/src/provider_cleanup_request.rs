use serde::{Deserialize, Serialize};

use crate::{CancelIntent, LeaseAttemptIdentity};

/// Asks the worker holding an attempt to clean up what the provider kept for it.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderCleanupRequest {
    pub identity: LeaseAttemptIdentity,
    pub intent: CancelIntent,
}
