use serde::{Deserialize, Serialize};

use crate::{LeaseError, StdinSeq};

/// A worker's answer to a [`WriteStdinRequest`](crate::WriteStdinRequest), tagged by `outcome`.
///
/// `next_seq` is the next `seq` the worker will apply. `Buffered` means the write arrived ahead of order and is held
/// until the gap fills. `OutOfWindow` means it is too far ahead to hold, and the caller should resend from `next_seq`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum WriteStdinResponse {
    Written { next_seq: StdinSeq },
    Duplicate { next_seq: StdinSeq },
    Buffered { next_seq: StdinSeq },
    Backpressure { next_seq: StdinSeq },
    OutOfWindow { next_seq: StdinSeq },
    StdinClosed,
    StdinNotInteractive,
    AlreadyCompleted,
    NotFound,
    Unsupported,
    Denied(LeaseError),
}
