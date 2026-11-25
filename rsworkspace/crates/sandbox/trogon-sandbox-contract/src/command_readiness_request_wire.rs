use serde::Deserialize;

use crate::{CommandReadinessTarget, LeaseId, ProviderKind, SandboxId, SandboxUserName, TenantId};

/// The JSON shape of a [`CommandReadinessRequest`](crate::CommandReadinessRequest) before its lease id is checked.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandReadinessRequestWire {
    pub(crate) tenant: TenantId,
    pub(crate) lease_id: LeaseId,
    pub(crate) provider: ProviderKind,
    pub(crate) target: CommandReadinessTarget,
    pub(crate) sandbox_id: SandboxId,
    pub(crate) run_as_user: Option<SandboxUserName>,
}
