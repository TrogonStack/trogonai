use serde::{Deserialize, Serialize};

/// What a channel runs.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SshChannelKind {
    /// The sandbox's default shell.
    Shell,
    /// One command line, run through `sh -c`.
    Exec { command: String },
}
