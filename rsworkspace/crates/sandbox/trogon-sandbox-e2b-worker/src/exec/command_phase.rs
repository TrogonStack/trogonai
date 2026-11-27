use serde::{Deserialize, Serialize};

/// How far a keyed command launch got.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CommandPhase {
    Reserved,
    Preparing,
    Bound,
    Running,
    Canceled,
}
