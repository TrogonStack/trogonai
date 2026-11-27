use serde::Deserialize;

use crate::{Operation, SpaceId};

/// SPACE's acknowledgement of a call it will finish asynchronously.
#[derive(Debug, Clone, Deserialize)]
pub struct Accepted {
    /// The operation to poll for the outcome.
    pub operation_id: SpaceId,
    /// The sandbox the call concerns, when SPACE names it.
    #[serde(default)]
    pub sandbox_id: Option<SpaceId>,
    /// The operation itself, when SPACE already knows how it ended.
    #[serde(default)]
    pub operation: Option<Operation>,
}
