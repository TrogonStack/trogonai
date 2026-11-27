use crate::fs_access_set::FsAccessSet;

/// The lists [`rank_fs_access`](crate::rank_fs_access) ranks against each other. Paths are concrete
/// (already expanded against the disk).
#[derive(Debug, Clone, Default)]
pub struct FsAccessLists {
    pub allow_read: Vec<String>,
    pub allow_write: Vec<String>,
    pub deny_read: Vec<String>,
    pub deny_write: Vec<String>,
    pub credential_deny: Vec<String>,
    /// Grants already on disk, which cannot be taken back for one command:
    /// each gets the deny at its own path.
    pub held: Option<FsAccessSet>,
}
