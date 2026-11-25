use serde::{Deserialize, Serialize};

/// Whether a command is bounded by its timeout or by its lease.
///
/// `Lease` requires a detached command. It serializes as the snake case variant name.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandDeadline {
    #[default]
    Timeout,
    Lease,
}
