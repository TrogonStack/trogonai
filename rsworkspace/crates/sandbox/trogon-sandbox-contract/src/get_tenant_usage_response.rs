use serde::{Deserialize, Serialize};

use crate::{LeaseError, TenantUsage};

/// The broker's answer to a [`GetTenantUsageRequest`](crate::GetTenantUsageRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum GetTenantUsageResponse {
    Usage { usage: TenantUsage },
    Denied(LeaseError),
}
