use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::PullSecretId;

use crate::CommittedPolicy;

/// The wire form of a committed policy in the KV bucket.
#[derive(Serialize, Deserialize)]
pub(crate) struct StoredPolicy {
    pub(crate) revision: u64,
    pub(crate) references: Vec<PullSecretId>,
}

impl StoredPolicy {
    pub(crate) fn into_committed(self) -> CommittedPolicy {
        CommittedPolicy {
            revision: self.revision,
            references: self.references.into_iter().collect(),
        }
    }
}
