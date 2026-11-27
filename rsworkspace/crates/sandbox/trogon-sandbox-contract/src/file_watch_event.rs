use serde::{Deserialize, Serialize};

use crate::{FileWatchEventKind, FileWatchTarget};

/// One change a watch observed.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileWatchEvent {
    /// Increases by one per event within a watch, and is the cursor for the next poll.
    pub seq: u64,
    pub kind: FileWatchEventKind,
    pub path: String,
    pub is_dir: FileWatchTarget,
}
