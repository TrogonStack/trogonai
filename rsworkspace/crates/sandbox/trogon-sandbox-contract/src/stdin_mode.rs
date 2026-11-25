use serde::{Deserialize, Serialize};

/// Whether a command's stdin stays open for [`WriteStdinRequest`](crate::WriteStdinRequest)s.
///
/// `Interactive` requires a detached command. It serializes as the snake case variant name.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StdinMode {
    #[default]
    Closed,
    Interactive,
}
