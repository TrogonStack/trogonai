use serde::{Deserialize, Serialize};

/// Outcome of a [`PurgeNamedRequest`](crate::PurgeNamedRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum PurgeNamedResponse {
    Purged,
    NotFound,
    Failed { message: String },
}
