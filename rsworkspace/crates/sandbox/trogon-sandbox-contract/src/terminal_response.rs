use serde::{Deserialize, Serialize};

use crate::{LeaseError, TerminalError, TerminalId};

/// The reply to a [`TerminalRequest`](crate::TerminalRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum TerminalResponse {
    /// The terminal is open and its output is flowing.
    Opened {
        terminal_id: TerminalId,
    },
    /// An input, resize, or close was applied.
    Done,
    Failed(TerminalError),
    /// The worker or provider has no terminal support.
    Unsupported,
    Denied(LeaseError),
}
