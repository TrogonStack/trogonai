use serde::{Deserialize, Serialize};

use crate::{LeaseError, LeaseId, LeaseSummary, ReapFailure, ReapMode};

/// Outcome of a [`ReapLeasesRequest`](crate::ReapLeasesRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum ReapLeasesResponse {
    Reaped {
        dry_run: ReapMode,
        /// Every lease that matched the filters.
        matched: Vec<LeaseSummary>,
        /// Leases actually released. Always empty on a dry run.
        released: Vec<LeaseId>,
        failed: Vec<ReapFailure>,
    },
    Denied(LeaseError),
}
