use serde::{Deserialize, Serialize};

use crate::DigestMismatch;

/// Why a worker failed a lease after it started, tagged by `kind`.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum LeaseFailedReason {
    DigestMismatch(DigestMismatch),
}
