use serde::{Deserialize, Serialize};

use crate::{CancelIntent, LeaseAttemptIdentity};

/// A worker's answer to a provider cleanup, tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum ProviderCleanupResponse {
    Cleaned {
        identity: LeaseAttemptIdentity,
        intent: CancelIntent,
    },
    Failed {
        identity: LeaseAttemptIdentity,
        message: String,
    },
}
