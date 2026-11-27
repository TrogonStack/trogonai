use serde::{Deserialize, Serialize};

use crate::TenantId;

/// A tenant's question about which operation features the broker offers.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationCapabilitiesRequest {
    pub tenant: TenantId,
}
