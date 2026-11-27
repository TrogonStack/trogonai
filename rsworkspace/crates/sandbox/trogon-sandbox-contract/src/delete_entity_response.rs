use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// The reply to a [`DeleteEntityRequest`](crate::DeleteEntityRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum DeleteEntityResponse {
    Deleted,
    NotFound,
    Denied(LeaseError),
}
