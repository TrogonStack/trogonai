use serde::{Deserialize, Serialize};

use crate::IdleAction;

/// One step of an [`IdlePolicy`](crate::IdlePolicy): apply `action` once the lease has been quiet
/// for more than `after_seconds`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct IdleStage {
    pub after_seconds: u32,
    pub action: IdleAction,
}
