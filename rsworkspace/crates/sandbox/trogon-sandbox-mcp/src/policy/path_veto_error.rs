/// Why a file path was refused by the root policy.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathVetoError {
    /// The path is empty.
    #[error("path is empty")]
    Empty,
    /// The path has a `..` component. Paths are never normalized across it, so a traversal attempt is refused
    /// instead of being resolved to wherever it would land.
    #[error("path {0:?} contains a `..` component")]
    ParentComponent(String),
    /// The path resolves outside every configured root.
    #[error("path {0:?} is outside the configured roots")]
    OutsideRoots(String),
    /// A component below the root is a symlink, which could point out of the root.
    #[error("{0:?} is a symlink, which could point outside the configured roots")]
    Symlink(String),
    /// The component could not be inspected, so confinement cannot be shown.
    #[error("cannot verify {path:?} stays inside the configured roots: {reason}")]
    Unverified { path: String, reason: String },
}
