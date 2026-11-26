use serde::{Deserialize, Serialize};

use crate::LeaseId;

/// A lease a reap matched but could not release.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReapFailure {
    pub lease_id: LeaseId,
    pub message: String,
}
