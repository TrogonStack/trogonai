use serde::{Deserialize, Serialize};

use crate::LeaseId;

/// Broker-to-worker request for a lease's [`ResourceGauges`](crate::ResourceGauges). Only the
/// worker holding `lease_id` replies.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct ResourceUsageRequest {
    pub lease_id: LeaseId,
}
