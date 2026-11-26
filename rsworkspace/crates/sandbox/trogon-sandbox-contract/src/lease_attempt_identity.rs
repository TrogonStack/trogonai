use serde::{Deserialize, Serialize};

use crate::{DispatchTarget, LeaseId, ProviderKind};

/// The lease, provider, and dispatch target one worker attempt runs under.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct LeaseAttemptIdentity {
    pub lease_id: LeaseId,
    pub provider: ProviderKind,
    pub target: DispatchTarget,
}
