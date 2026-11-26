use serde::{Deserialize, Serialize};

use crate::ExecutionId;

/// Published once by a worker after a streamed command exits and its last
/// [`CommandOutputChunk`](crate::CommandOutputChunk) was published.
///
/// A log follower can end as soon as the command does instead of polling.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandExitMarker {
    pub execution_id: ExecutionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}
