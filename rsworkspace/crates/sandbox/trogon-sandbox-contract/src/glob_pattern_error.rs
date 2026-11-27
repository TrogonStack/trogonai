/// A string is not a path pattern [`GlobPattern`](crate::GlobPattern) accepts.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum GlobPatternError {
    #[error("pattern cannot be empty")]
    Empty,
    #[error("pattern {0:?} must be absolute")]
    NotAbsolute(String),
    #[error("pattern contains a NUL byte")]
    ContainsNul,
    #[error("pattern is longer than {max} bytes")]
    TooLong { max: usize },
    #[error("pattern {0:?} climbs above the root with \"..\"")]
    EscapesRoot(String),
    #[error("pattern {0:?} has a \"..\" that follows a wildcard segment and cannot be folded")]
    AmbiguousParent(String),
    #[error("{0:?} has no wildcard; declare it as a literal path")]
    NotAGlob(String),
}
