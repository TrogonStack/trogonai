use serde::{Deserialize, Serialize};

use crate::Base64Data;

/// One frame of a terminal's output stream, tagged by `type`.
///
/// `seq` starts at 0 and has no gaps unless a slow reader lost frames. A terminal ends with exactly
/// one `exit` frame.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum TerminalOutput {
    /// Bytes the terminal produced, with stdout and stderr merged. `data` is base64.
    Data { seq: u64, data: Base64Data },
    /// The terminal ended. `exit_code` is absent when the process was killed by a signal or the code is
    /// unknown.
    Exit {
        seq: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
    },
    /// Sent on a reattach when the caller's `resume_from` is older than what the worker still holds.
    /// Output before `from_seq` is gone for good, so the caller should discard its screen state.
    Resync { from_seq: u64 },
}
