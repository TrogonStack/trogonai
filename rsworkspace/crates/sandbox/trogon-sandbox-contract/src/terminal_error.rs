use serde::{Deserialize, Serialize};

/// Why a terminal operation failed after it was admitted, tagged by `kind`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TerminalError {
    /// No open terminal has this id on this lease.
    #[error("terminal not found")]
    NotFound,
    /// The id is open on another lease. Ids are not shared across leases.
    #[error("terminal id is in use")]
    IdInUse,
    #[error("the lease already has {max} open terminals")]
    TooManyTerminals { max: u32 },
    #[error("invalid terminal request: {reason}")]
    InvalidRequest { reason: String },
    #[error("terminal could not be started: {reason}")]
    SpawnFailed { reason: String },
    /// The terminal's process has exited; only closing it is useful now.
    #[error("terminal has exited")]
    Exited,
}
