use serde::{Deserialize, Serialize};

/// How a watched path changed.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileWatchEventKind {
    Created,
    Modified,
    Deleted,
}
