use serde::{Deserialize, Serialize};

use crate::{AccountLimits, LeaseError};

/// The broker's answer to a [`GetAccountLimitsRequest`](crate::GetAccountLimitsRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum GetAccountLimitsResponse {
    Limits { limits: AccountLimits },
    Denied(LeaseError),
}
