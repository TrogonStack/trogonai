use serde::{Deserialize, Serialize};

/// An operation's wire kind; runtime support is defined by its executor.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Create,
    Restore,
    Fork,
    Snapshot,
    Pause,
    Resume,
    Suspend,
    Wake,
    Stop,
    Delete,
    Rollback,
}

impl OperationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Restore => "restore",
            Self::Fork => "fork",
            Self::Snapshot => "snapshot",
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Suspend => "suspend",
            Self::Wake => "wake",
            Self::Stop => "stop",
            Self::Delete => "delete",
            Self::Rollback => "rollback",
        }
    }
}
