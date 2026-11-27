use trogon_sandbox_contract::{FileWatchEventKind, FileWatchTarget};

/// One change a live feed (such as inotify) saw under a watched tree.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ChangeHint {
    pub path: String,
    pub kind: FileWatchEventKind,
    pub target: FileWatchTarget,
}
