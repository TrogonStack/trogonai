/// Net difference between two manifests, each list in path order.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ManifestDiff {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
}

/// Whether `path` is `prefix` or lies under it.
pub(crate) fn is_within(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    prefix.is_empty() || path == prefix || path.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('/'))
}

impl ManifestDiff {
    /// Keeps only the paths at or under `prefix`.
    pub fn restrict(&mut self, prefix: &str) {
        self.retain(|path| is_within(path, prefix));
    }

    /// Drops the paths at or under `prefix`.
    pub(crate) fn exclude(&mut self, prefix: &str) {
        self.retain(|path| !is_within(path, prefix));
    }

    /// Adds `other`'s paths, keeping each list sorted and free of duplicates.
    pub(crate) fn merge(&mut self, other: Self) {
        for (list, extra) in [
            (&mut self.added, other.added),
            (&mut self.modified, other.modified),
            (&mut self.deleted, other.deleted),
        ] {
            list.extend(extra);
            list.sort();
            list.dedup();
        }
    }

    fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.added.retain(|path| keep(path));
        self.modified.retain(|path| keep(path));
        self.deleted.retain(|path| keep(path));
    }
}
