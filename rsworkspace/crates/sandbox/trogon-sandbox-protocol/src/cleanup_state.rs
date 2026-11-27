use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{CancelIntent, LeaseAttemptIdentity, ProviderLease};

use crate::AttemptPhase;

/// The durable record of one dispatch attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupState {
    /// The lease, provider and target the attempt belongs to.
    pub identity: LeaseAttemptIdentity,
    /// Whether the backend allocation was ever started, so a teardown has something to undo.
    pub allocation_started: bool,
    /// What the backend returned, until the teardown clears it.
    pub receipt: Option<ProviderLease>,
    /// Where the attempt stands.
    pub phase: AttemptPhase,
    /// Whether the cleanup keeps or destroys the lease's data.
    #[serde(default)]
    pub intent: CancelIntent,
    /// Whether the backend teardown finished.
    #[serde(default)]
    pub backend_cleaned: bool,
}
