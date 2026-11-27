use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::Digest;

use crate::GuestEntropyReseedMethod;

/// Proof that a seed was applied, naming it by digest rather than by value.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct GuestEntropyReseedReceipt {
    pub seed_digest: Digest,
    pub method: GuestEntropyReseedMethod,
}
