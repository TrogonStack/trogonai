//! Which service answers a structured file operation.

use trogon_sandbox_contract::FileOp;

/// How the provider routes a [`FileOp`]: plain verbs run in the container,
/// change-tracking and watch verbs go through the lease's tracking state.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum FileOpClass {
    Plain,
    ChangeTracking,
    Watch,
}

impl FileOpClass {
    pub(crate) fn of(op: &FileOp) -> Self {
        match op {
            FileOp::Checkpoint { .. } | FileOp::Changes { .. } => Self::ChangeTracking,
            FileOp::WatchStart { .. } | FileOp::WatchPoll { .. } | FileOp::WatchStop { .. } => Self::Watch,
            _ => Self::Plain,
        }
    }

    pub(crate) fn is_tracked(self) -> bool {
        !matches!(self, Self::Plain)
    }

    pub(crate) fn is_watch(self) -> bool {
        matches!(self, Self::Watch)
    }
}
