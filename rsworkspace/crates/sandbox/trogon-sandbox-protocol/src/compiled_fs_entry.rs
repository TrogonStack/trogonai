use trogon_sandbox_contract::{FsPathEntry, LiteralPath};

use crate::{GlobMatcher, LiteralPathExt};

/// An [`FsPathEntry`] ready to be asked about paths.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum CompiledFsEntry {
    /// A name, covering itself and everything beneath it.
    Literal(LiteralPath),
    /// A pattern, covering every path it matches and everything beneath such a path.
    Glob {
        /// The compiled pattern.
        matcher: GlobMatcher,
        /// The path that stands for the pattern when it is compared with another entry, when one can be named.
        sample: Option<LiteralPath>,
    },
}

impl CompiledFsEntry {
    /// Compiles `entry`.
    pub fn new(entry: &FsPathEntry) -> Self {
        match entry {
            FsPathEntry::Literal { path, .. } => Self::Literal(path.clone()),
            FsPathEntry::Glob { pattern } => {
                let matcher = GlobMatcher::new(pattern);
                let sample = matcher.sample_path().ok();
                Self::Glob { matcher, sample }
            }
        }
    }

    /// Reports whether the entry names `path` or a directory above it, or a pattern matches the path or a directory
    /// above it.
    pub fn covers(&self, path: &LiteralPath) -> bool {
        match self {
            Self::Literal(entry) => path.is_within(entry),
            Self::Glob { matcher, .. } => matcher.covers(path),
        }
    }

    /// The path that stands for the entry when it is compared with a pattern: the path itself, or the sample of a
    /// pattern. A pattern whose sample cannot be named has none.
    pub fn sample_path(&self) -> Option<&LiteralPath> {
        match self {
            Self::Literal(path) => Some(path),
            Self::Glob { sample, .. } => sample.as_ref(),
        }
    }
}
