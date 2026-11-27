use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::LeaseId;

use crate::GuestEntropyReseedReceipt;

/// The answer to a bind that carried an entropy seed.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct BindLeaseResponse {
    pub lease_id: LeaseId,
    pub entropy_reseed: GuestEntropyReseedReceipt,
}
