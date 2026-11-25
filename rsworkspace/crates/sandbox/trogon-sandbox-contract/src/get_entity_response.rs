use serde::{Deserialize, Serialize};

use crate::{LeaseError, SandboxEntityRow};

/// The reply to a [`GetEntityRequest`](crate::GetEntityRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum GetEntityResponse {
    Found {
        row: SandboxEntityRow,
    },
    /// No such entity for the tenant. An id owned by another tenant answers the same.
    NotFound,
    Denied(LeaseError),
}
