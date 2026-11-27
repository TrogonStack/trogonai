use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::TerminalDimension;

use crate::SshChannelKind;

/// Control messages on a session's stream. `Open` and `Resize` travel to the worker; `Exit` and `Error` travel back.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "op")]
pub enum SshStreamControl {
    /// Starts the channel's process. Sent once per channel, before any data.
    Open {
        kind: SshChannelKind,
        cols: TerminalDimension,
        rows: TerminalDimension,
        /// The client's `TERM`, when it requested a pty.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        term: Option<String>,
    },
    /// The client changed its window size.
    Resize {
        cols: TerminalDimension,
        rows: TerminalDimension,
    },
    /// The channel's process ended. `status` is `None` when it was killed by a signal or the status is unknown.
    /// Followed by an `eof` frame.
    Exit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<u32>,
    },
    /// The channel could not be served. Followed by an `eof` frame.
    Error { message: String },
}
