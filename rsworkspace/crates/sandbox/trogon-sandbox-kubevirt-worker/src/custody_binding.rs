use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::WorkerId;

use crate::BoundedName;

/// The worker and namespace a custody directory belongs to.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CustodyBinding {
    pub(crate) version: u32,
    pub(crate) worker: WorkerId,
    pub(crate) namespace: BoundedName,
}
