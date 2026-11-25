use serde::{Deserialize, Serialize};

use crate::{ExecutionId, OutputStream};

/// One incremental chunk of a streamed command's output.
///
/// `seq` increases monotonically per execution and stream, so a consumer can detect gaps and reordering.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandOutputChunk {
    pub execution_id: ExecutionId,
    pub stream: OutputStream,
    pub seq: u64,
    pub data: Vec<u8>,
}
