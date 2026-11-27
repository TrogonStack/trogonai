use trogon_sandbox_contract::{FileSha256, FileWatchTarget};

/// What a worker can say about one path, cheaply enough to compare two trees.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ManifestEntry {
    target: FileWatchTarget,
    size: u64,
    stamp: Option<String>,
    hash: Option<FileSha256>,
}

impl ManifestEntry {
    /// A file, with an opaque modification stamp (equal stamps mean "not rewritten") and a content
    /// hash when the manifest was taken with hashing on.
    pub fn file(size: u64, stamp: Option<String>, hash: Option<FileSha256>) -> Self {
        Self {
            target: FileWatchTarget::File,
            size,
            stamp,
            hash,
        }
    }

    /// A directory. It carries no size, stamp, or hash, because its stamp moves whenever a child
    /// does.
    pub fn directory() -> Self {
        Self {
            target: FileWatchTarget::Directory,
            size: 0,
            stamp: None,
            hash: None,
        }
    }

    pub fn target(&self) -> FileWatchTarget {
        self.target
    }

    pub fn is_directory(&self) -> bool {
        self.target.is_directory()
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn stamp(&self) -> Option<&str> {
        self.stamp.as_deref()
    }

    pub fn hash(&self) -> Option<&FileSha256> {
        self.hash.as_ref()
    }

    pub(crate) fn changed_from(&self, before: &Self) -> bool {
        if before.target != self.target {
            return true;
        }
        if self.is_directory() {
            return false;
        }
        if before.size != self.size {
            return true;
        }
        match (&before.hash, &self.hash) {
            (Some(a), Some(b)) => a != b,
            _ => before.stamp != self.stamp,
        }
    }
}
