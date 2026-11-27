use serde::{Deserialize, Serialize};

use crate::{SandboxEntityId, TenantId};

/// Client request to list a tenant's entities, ordered by id and paged by the last id of the previous page.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ListEntitiesRequest {
    pub tenant: TenantId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<SandboxEntityId>,
}

impl ListEntitiesRequest {
    /// Most rows one request returns.
    pub const MAX_LIMIT: u32 = 500;
    /// Rows returned when `limit` is absent.
    pub const DEFAULT_LIMIT: u32 = 100;

    /// The page size: `limit` or [`DEFAULT_LIMIT`](Self::DEFAULT_LIMIT), at least one and at most
    /// [`MAX_LIMIT`](Self::MAX_LIMIT).
    pub fn page_size(&self) -> u32 {
        self.limit.unwrap_or(Self::DEFAULT_LIMIT).clamp(1, Self::MAX_LIMIT)
    }
}
