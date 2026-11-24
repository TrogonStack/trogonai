use std::time::SystemTime;

use crate::{HopKind, WorkloadId};

/// A JWT-SVID that passed every verification rule. It can only be produced by
/// [`crate::JwtSvidVerifier::verify`]; the token and its raw claims are not
/// retained, so there is nothing to forward and no claim to read unchecked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedJwtSvid {
    workload_id: WorkloadId,
    audience: String,
    expires_at: SystemTime,
    hop: HopKind,
}

impl VerifiedJwtSvid {
    pub(crate) fn new(workload_id: WorkloadId, audience: String, expires_at: SystemTime, hop: HopKind) -> Self {
        Self {
            workload_id,
            audience,
            expires_at,
            hop,
        }
    }

    pub fn workload_id(&self) -> &WorkloadId {
        &self.workload_id
    }

    /// The verifier's expected audience, which the token was proven to carry.
    pub fn audience(&self) -> &str {
        &self.audience
    }

    pub fn expires_at(&self) -> SystemTime {
        self.expires_at
    }

    /// How the token reached the verifier, for logging.
    pub fn hop(&self) -> HopKind {
        self.hop
    }
}
