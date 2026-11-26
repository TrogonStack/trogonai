use serde::{Deserialize, Serialize};

use crate::{LeaseError, LeaseListRow};

/// Outcome of a [`GetLeaseByNameRequest`](crate::GetLeaseByNameRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum GetLeaseByNameResponse {
    Found {
        row: LeaseListRow,
    },
    NotFound,
    /// The caller may not read the tenant's leases. Kept apart from `NotFound` so a refusal is not mistaken for a free
    /// name.
    Denied(LeaseError),
}
