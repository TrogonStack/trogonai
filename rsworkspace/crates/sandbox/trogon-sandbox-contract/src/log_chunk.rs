use serde::{Deserialize, Serialize};

use crate::OutputStream;

/// One retained output chunk.
///
/// `log_seq` orders the chunks of both streams of one execution and is the cursor for
/// [`ReadLogsRequest::after_seq`](crate::ReadLogsRequest::after_seq). `seq` is the worker's own
/// [`CommandOutputChunk::seq`](crate::CommandOutputChunk::seq).
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct LogChunk {
    pub log_seq: u64,
    pub stream: OutputStream,
    pub seq: u64,
    pub data: Vec<u8>,
}
