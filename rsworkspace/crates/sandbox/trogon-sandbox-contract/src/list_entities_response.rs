use serde::{Deserialize, Serialize};

use crate::{LeaseError, SandboxEntityId, SandboxEntityRow};

/// The reply to a [`ListEntitiesRequest`](crate::ListEntitiesRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum ListEntitiesResponse {
    Listed {
        rows: Vec<SandboxEntityRow>,
        /// Pass as `cursor` to read the next page. Absent on the last page.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_cursor: Option<SandboxEntityId>,
    },
    Denied(LeaseError),
}
