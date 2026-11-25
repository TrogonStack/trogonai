use serde::{Deserialize, Serialize};

use crate::{LeaseError, LeaseUsage};

/// The broker's answer to a [`GetLeaseUsageRequest`](crate::GetLeaseUsageRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum GetLeaseUsageResponse {
    Usage { usage: LeaseUsage },
    Denied(LeaseError),
}
