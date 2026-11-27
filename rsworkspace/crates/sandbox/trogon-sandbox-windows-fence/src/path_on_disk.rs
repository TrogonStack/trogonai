/// What looking at a path on disk found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathOnDisk {
    /// The object it names, with links, 8.3 names and case resolved.
    Resolved(String),
    Absent,
    /// Never looked at: a share or the device namespace.
    Unprobed,
    /// Looking failed for another reason: access denied, an I/O error.
    Failed(String),
}
