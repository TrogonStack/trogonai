/// The verdict of a file system policy for one path.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FsAccess {
    /// The operation is permitted.
    Allowed,
    /// The operation is refused.
    Denied,
}
