use trogon_sandbox_contract::{FileOp, LiteralPath};
use trogon_sandbox_protocol::LiteralPathExt;

use crate::constants::{TMP_GUEST_DIR, WORKSPACE_GUEST_DIR};

/// A guest directory mounted from a host directory of the lease: the only places in a hluk guest that outlive a
/// command.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GuestDir {
    /// `/workspace`, the lease's workspace and the default for paths that name neither directory.
    Workspace,
    /// `/tmp`, the lease's scratch space.
    Tmp,
}

impl GuestDir {
    /// Where this directory appears in the guest.
    pub fn guest_path(self) -> &'static str {
        match self {
            Self::Workspace => WORKSPACE_GUEST_DIR,
            Self::Tmp => TMP_GUEST_DIR,
        }
    }

    /// The directory `path` falls in once `.` and `..` fold. A relative path, or one outside both, is left to the
    /// workspace, which resolves or refuses it.
    pub fn of(path: &str) -> Self {
        let in_tmp = LiteralPath::new(path)
            .is_ok_and(|path| LiteralPath::new(TMP_GUEST_DIR).is_ok_and(|tmp| path.is_within(&tmp)));
        if in_tmp { Self::Tmp } else { Self::Workspace }
    }

    /// Every directory, in mount order.
    pub const ALL: [Self; 2] = [Self::Workspace, Self::Tmp];

    /// The one directory every path `op` names falls in, or `None` when they span both.
    pub fn of_op(op: &FileOp) -> Option<Self> {
        match op {
            FileOp::Rename { from, to, .. } => Some(Self::of(from)).filter(|dir| *dir == Self::of(to)),
            FileOp::Changes { path, .. } => Some(path.as_deref().map_or(Self::Workspace, Self::of)),
            // The profile declares no file watch, so these name a watch that does not exist on either root.
            FileOp::WatchPoll { .. } | FileOp::WatchStop { .. } => Some(Self::Workspace),
            FileOp::List { path, .. }
            | FileOp::Stat { path }
            | FileOp::Read { path, .. }
            | FileOp::Write { path, .. }
            | FileOp::Edit { path, .. }
            | FileOp::Search { path, .. }
            | FileOp::Remove { path, .. }
            | FileOp::Mkdir { path, .. }
            | FileOp::ArchiveGet { path, .. }
            | FileOp::ArchivePut { path, .. }
            | FileOp::Checkpoint { path, .. }
            | FileOp::WatchStart { path, .. }
            | FileOp::Checksum { path, .. } => Some(Self::of(path)),
        }
    }
}

#[cfg(test)]
mod tests {
    use trogon_sandbox_contract::FileOverwrite;

    use super::*;

    #[test]
    fn only_tmp_and_paths_under_it_fall_in_tmp() {
        assert_eq!(GuestDir::of("/tmp"), GuestDir::Tmp);
        assert_eq!(GuestDir::of("/tmp/a/b"), GuestDir::Tmp);
        assert_eq!(GuestDir::of("/tmpfoo"), GuestDir::Workspace);
        assert_eq!(GuestDir::of("/workspace/a"), GuestDir::Workspace);
        assert_eq!(GuestDir::of("relative"), GuestDir::Workspace);
        assert_eq!(GuestDir::of("/etc/passwd"), GuestDir::Workspace);
        assert_eq!(GuestDir::of("/workspace/../tmp/a"), GuestDir::Tmp);
        assert_eq!(GuestDir::of("/tmp/../workspace/a"), GuestDir::Workspace);
    }

    #[test]
    fn a_rename_across_the_two_directories_has_no_single_directory() {
        let rename = |from: &str, to: &str| FileOp::Rename {
            from: from.to_owned(),
            to: to.to_owned(),
            overwrite: FileOverwrite::default(),
        };
        assert_eq!(GuestDir::of_op(&rename("/tmp/a", "/tmp/b")), Some(GuestDir::Tmp));
        assert_eq!(GuestDir::of_op(&rename("/tmp/a", "/workspace/b")), None);
    }
}
