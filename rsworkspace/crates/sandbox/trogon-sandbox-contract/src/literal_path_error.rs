/// A string is not a path [`LiteralPath`](crate::LiteralPath) accepts.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum LiteralPathError {
    #[error("path cannot be empty")]
    Empty,
    #[error("path {0:?} must be absolute")]
    NotAbsolute(String),
    #[error("path contains a NUL byte")]
    ContainsNul,
    #[error("path is longer than {max} bytes")]
    TooLong { max: usize },
    #[error("path {0:?} climbs above the root with \"..\"")]
    EscapesRoot(String),
}
