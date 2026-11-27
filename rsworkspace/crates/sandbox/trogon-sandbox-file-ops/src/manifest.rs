use std::collections::BTreeMap;
use std::collections::btree_map;

use crate::{ManifestDiff, ManifestEntry};

/// Paths, as the sandbox addresses them, mapped to their fingerprints in path order.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct Manifest(BTreeMap<String, ManifestEntry>);

impl Manifest {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses `stat -c '%F|%s|%y|%n'` lines. A name may contain `|`, so the name is the rest.
    /// Lines that do not have four fields are skipped.
    pub fn from_stat_output(text: &str) -> Self {
        let mut out = Self::new();
        for line in text.lines() {
            let mut fields = line.splitn(4, '|');
            let (Some(kind), Some(size), Some(stamp), Some(name)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let entry = if kind.contains("directory") {
                ManifestEntry::directory()
            } else {
                ManifestEntry::file(size.trim().parse().unwrap_or(0), Some(stamp.to_string()), None)
            };
            out.insert(name.to_string(), entry);
        }
        out
    }

    pub fn insert(&mut self, path: impl Into<String>, entry: ManifestEntry) -> Option<ManifestEntry> {
        self.0.insert(path.into(), entry)
    }

    pub fn get(&self, path: &str) -> Option<&ManifestEntry> {
        self.0.get(path)
    }

    pub fn contains_path(&self, path: &str) -> bool {
        self.0.contains_key(path)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> btree_map::Iter<'_, String, ManifestEntry> {
        self.0.iter()
    }

    /// Compares this manifest, taken earlier, with `after`. A path whose kind flipped between
    /// file and directory counts as modified, since the path exists in both.
    pub fn diff(&self, after: &Self) -> ManifestDiff {
        let mut out = ManifestDiff::default();
        for (path, entry) in &after.0 {
            match self.0.get(path) {
                None => out.added.push(path.clone()),
                Some(old) if entry.changed_from(old) => out.modified.push(path.clone()),
                Some(_) => {}
            }
        }
        for path in self.0.keys() {
            if !after.0.contains_key(path) {
                out.deleted.push(path.clone());
            }
        }
        out
    }
}

impl FromIterator<(String, ManifestEntry)> for Manifest {
    fn from_iter<T: IntoIterator<Item = (String, ManifestEntry)>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl<'a> IntoIterator for &'a Manifest {
    type Item = (&'a String, &'a ManifestEntry);
    type IntoIter = btree_map::Iter<'a, String, ManifestEntry>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}
