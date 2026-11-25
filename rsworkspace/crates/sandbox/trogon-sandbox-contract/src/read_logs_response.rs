use serde::{Deserialize, Serialize};

use crate::{CommandExitReport, LeaseError, LogChunk, LogsContinuation, OutputTruncation};

/// A worker's answer to a [`ReadLogsRequest`](crate::ReadLogsRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum ReadLogsResponse {
    /// `next_seq` is the `after_seq` for the following page, the request's own when no chunk was returned.
    /// `has_more` is set when the page stopped at `max_bytes` with retained chunks left. `truncated` is set when
    /// retention already dropped the start of this execution's output.
    Logs {
        chunks: Vec<LogChunk>,
        next_seq: u64,
        #[serde(default)]
        has_more: LogsContinuation,
        #[serde(default)]
        truncated: OutputTruncation,
        /// The worker reported the command's exit, so once a page returns no chunk and no `has_more` there is
        /// nothing left to follow. Brokers and workers that predate the exit marker leave it unreported, and
        /// followers then fall back to polling the command.
        #[serde(default)]
        exited: CommandExitReport,
    },
    /// This deployment keeps no command logs.
    Unsupported,
    Denied(LeaseError),
}
