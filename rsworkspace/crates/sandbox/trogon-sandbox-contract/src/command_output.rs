use serde::{Deserialize, Serialize};

use crate::{CommandTimeoutStatus, OutputTruncation};

/// What a command left behind when it ended.
///
/// An omitted `timed_out` or `truncated` reads as the cleared state, and the cleared state is left off the wire.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// The command exceeded its deadline and was killed; `exit_code` is [`CommandOutput::TIMED_OUT_EXIT_CODE`].
    #[serde(default, skip_serializing_if = "CommandTimeoutStatus::is_within_deadline")]
    pub timed_out: CommandTimeoutStatus,
    /// Stdout or stderr exceeded the retention cap and its middle was dropped.
    #[serde(default, skip_serializing_if = "OutputTruncation::is_complete")]
    pub truncated: OutputTruncation,
}

impl CommandOutput {
    /// The `exit_code` every worker reports alongside a timeout, so a caller can tell a deadline kill from a command
    /// that exited on its own without knowing which provider ran it.
    pub const TIMED_OUT_EXIT_CODE: i32 = -1;
}
