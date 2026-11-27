use std::sync::Arc;

use trogon_sandbox_contract::TenantId;
use trogon_sandbox_protocol::{AuthorityResourceId, AuthorityTaskCompletion};

use crate::{MaterializedWorkspace, SessionIdentifier};

/// A session a lease still holds.
#[derive(Clone)]
pub(crate) struct ActiveAcaLease {
    pub(crate) tenant: TenantId,
    pub(crate) session: SessionIdentifier,
    pub(crate) resource: AuthorityResourceId,
    pub(crate) completion: AuthorityTaskCompletion,
    pub(crate) workspace: Option<Arc<MaterializedWorkspace>>,
    pub(crate) volume_synced: bool,
    pub(crate) cleanup_requested: bool,
}
