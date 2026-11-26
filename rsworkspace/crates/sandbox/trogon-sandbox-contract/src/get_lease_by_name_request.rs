use serde::{Deserialize, Serialize};

use crate::{LeaseName, TenantId};

/// Client-facing request to look up a running lease's [`LeaseListRow`](crate::LeaseListRow) by its tenant-scoped name.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetLeaseByNameRequest {
    pub tenant: TenantId,
    pub name: LeaseName,
}
