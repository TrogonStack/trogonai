use std::fmt;
use std::path::PathBuf;

/// Where a policy snapshot came from. Every audit event names it, so a
/// decision made on an operator-authored file cannot be mistaken for one made
/// on a live projection (ADR#0085 Decision 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicySource {
    /// An operator-authored file. Not a system of record, no revocation clock.
    StaticFile { path: PathBuf },
}

impl PolicySource {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::StaticFile { .. } => "static_file",
        }
    }
}

impl fmt::Display for PolicySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaticFile { path } => write!(f, "static_file:{}", path.display()),
        }
    }
}
