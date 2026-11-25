use serde::{Deserialize, Serialize};

use crate::TenantId;

/// Provider-facing request to purge a named sandbox's suspended actor by tenant and name.
///
/// The broker sends it after any active lease and snapshot tag under the name have already been dealt with. It is
/// scoped by name rather than lease id because the actor can outlive every lease that ever held it.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PurgeNamedRequest {
    pub tenant: TenantId,
    pub name: String,
}
