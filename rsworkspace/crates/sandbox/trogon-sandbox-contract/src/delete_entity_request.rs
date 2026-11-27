use serde::{Deserialize, Serialize};

use crate::{EntityRef, TenantId};

/// Client request to permanently delete an entity: its running lease, its snapshots, and its name.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeleteEntityRequest {
    pub tenant: TenantId,
    pub entity: EntityRef,
}
