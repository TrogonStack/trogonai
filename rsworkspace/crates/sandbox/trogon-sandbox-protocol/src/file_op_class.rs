use trogon_sandbox_contract::FileOp;

/// Which family of verbs a [`FileOp`] belongs to.
///
/// A provider profile advertises the integrity, change-tracking, and watch verbs separately from the plain file
/// verbs.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum FileOpClass {
    /// A plain file verb: list, stat, read, write, edit, search, remove, mkdir, rename, or an archive verb.
    Plain,
    /// The integrity verb a directory transfer relies on.
    DirTransfer,
    /// A checkpoint or a changes report.
    ChangeTracking,
    /// Starting, polling, or stopping a watch.
    FileWatch,
}

impl FileOpClass {
    /// The family `op` belongs to.
    pub fn of(op: &FileOp) -> Self {
        match op {
            FileOp::Checksum { .. } => Self::DirTransfer,
            FileOp::Checkpoint { .. } | FileOp::Changes { .. } => Self::ChangeTracking,
            FileOp::WatchStart { .. } | FileOp::WatchPoll { .. } | FileOp::WatchStop { .. } => Self::FileWatch,
            FileOp::List { .. }
            | FileOp::Stat { .. }
            | FileOp::Read { .. }
            | FileOp::Write { .. }
            | FileOp::Edit { .. }
            | FileOp::Search { .. }
            | FileOp::Remove { .. }
            | FileOp::Mkdir { .. }
            | FileOp::Rename { .. }
            | FileOp::ArchiveGet { .. }
            | FileOp::ArchivePut { .. } => Self::Plain,
        }
    }
}
