use std::collections::BTreeMap;
use std::num::NonZeroU16;

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::LeaseId;

use crate::GuestEntropySeed;

/// Binds a freshly resumed actor to the lease it is serving.
///
/// Per-lease command and environment are deliberately absent from the actor template, which is content-addressed and
/// shared across leases, so this call is what makes a generic resumed actor into this lease's sandbox.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct BindLeaseRequest {
    pub lease_id: LeaseId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entropy_seed: Option<GuestEntropySeed>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposed_port: Option<NonZeroU16>,
}
