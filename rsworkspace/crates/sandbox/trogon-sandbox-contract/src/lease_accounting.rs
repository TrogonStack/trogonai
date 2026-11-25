use serde::{Deserialize, Serialize};

/// Which pool a lease's usage is charged to.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseAccounting {
    Tenant,
    Prewarm,
}
