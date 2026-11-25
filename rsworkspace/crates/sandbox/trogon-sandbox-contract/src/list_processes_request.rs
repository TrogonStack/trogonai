use serde::{Deserialize, Serialize};

use crate::LeaseId;

/// A request to list the named or probed processes of a lease, running and exited.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ListProcessesRequest {
    pub lease_id: LeaseId,
}
