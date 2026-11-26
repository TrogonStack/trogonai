use serde::{Deserialize, Serialize};

use crate::{ReapMode, SandboxTags, TenantId};

/// The JSON shape of a [`ReapLeasesRequest`](crate::ReapLeasesRequest) before its filter rule is checked.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ReapLeasesRequestWire {
    pub(crate) tenant: TenantId,
    #[serde(default, skip_serializing_if = "SandboxTags::is_empty")]
    pub(crate) tags: SandboxTags,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) older_than_seconds: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) idle_for_seconds: Option<u32>,
    #[serde(default)]
    pub(crate) dry_run: ReapMode,
}
