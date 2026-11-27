use serde::{Deserialize, Serialize};

use crate::{EntityRef, TenantId};

/// Client request to read one entity, including a stopped one.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetEntityRequest {
    pub tenant: TenantId,
    pub entity: EntityRef,
}
