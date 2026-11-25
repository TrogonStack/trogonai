use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::IdleAction;

/// One step of an [`IdlePolicy`](crate::IdlePolicy): apply `action` once the lease has been quiet
/// for more than `after_seconds`, which is never zero.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct IdleStage {
    pub after_seconds: NonZeroU32,
    pub action: IdleAction,
}
