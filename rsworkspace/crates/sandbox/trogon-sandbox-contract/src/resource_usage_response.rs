use serde::{Deserialize, Serialize};

use crate::{LeaseError, ResourceGauges};

/// A worker's answer to a [`ResourceUsageRequest`](crate::ResourceUsageRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum ResourceUsageResponse {
    Gauges(ResourceGauges),
    Unsupported,
    Denied(LeaseError),
}
