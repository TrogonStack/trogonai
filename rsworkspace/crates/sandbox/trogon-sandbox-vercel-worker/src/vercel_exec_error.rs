use crate::VercelApiError;

/// Why a per-lease command, file or capture operation failed.
#[derive(Debug, thiserror::Error)]
pub enum VercelExecError {
    #[error(transparent)]
    Api(#[from] VercelApiError),
    /// The capture machinery returned something other than what it was asked for.
    #[error("{0}")]
    Capture(&'static str),
    /// The command stream reported an error of its own.
    #[error("{0}")]
    Stream(String),
    #[error("foreground command outcome is unknown: the native command ID was not received before the deadline")]
    CommandIdMissing,
    #[error("command {command_id} outcome is unknown: deadline termination failed: {source}")]
    DeadlineKillFailed {
        command_id: String,
        #[source]
        source: VercelApiError,
    },
    #[error("command {command_id} outcome is unknown: termination could not be confirmed: {source}")]
    TerminationUnconfirmed {
        command_id: String,
        #[source]
        source: VercelApiError,
    },
    #[error("command {command_id} outcome is unknown: deadline termination has no terminal receipt")]
    NoTerminalReceipt { command_id: String },
    #[error("command output recovery did not settle within the log read deadline")]
    RecoveryTimedOut,
}
