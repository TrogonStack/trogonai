use serde::{Deserialize, Serialize};

use crate::{LeaseError, LeaseGranted};

/// The reply to a lease request, tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum LeaseResponse {
    Granted(LeaseGranted),
    Denied(LeaseError),
}
