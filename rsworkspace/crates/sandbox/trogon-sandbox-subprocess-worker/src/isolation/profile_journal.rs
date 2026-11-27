//! The AppContainer profiles this worker created, kept as one marker file per
//! profile under the workspace root. A worker that crashed leaves its markers
//! behind, so the next one removes exactly those profiles and no other, which
//! never needs a listing of the profiles of the machine.

use std::io;
use std::path::{Path, PathBuf};

pub use super::constants::NAME_PREFIX;
use super::constants::{DIRECTORY, GENERATED_SUFFIX_LEN};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileName(String);

impl ProfileName {
    pub fn generate() -> Self {
        Self(format!("{NAME_PREFIX}{}", uuid::Uuid::now_v7().simple()))
    }

    /// Only a name this worker would have generated is accepted.
    pub fn parse(text: &str) -> Option<Self> {
        let id = text.strip_prefix(NAME_PREFIX)?;
        is_generated_suffix(id).then_some(())?;
        Some(Self(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A generated name carries a simple-format UUID after the prefix.
fn is_generated_suffix(id: &str) -> bool {
    id.len() == GENERATED_SUFFIX_LEN && uuid::Uuid::try_parse(id).is_ok()
}

#[derive(Debug, Clone)]
pub struct ProfileJournal {
    directory: PathBuf,
}

impl ProfileJournal {
    pub fn in_root(workspace_root: &Path) -> Self {
        Self {
            directory: workspace_root.join(DIRECTORY),
        }
    }

    pub fn record(&self, name: &ProfileName) -> io::Result<()> {
        std::fs::create_dir_all(&self.directory)?;
        std::fs::write(self.directory.join(name.as_str()), b"")
    }

    pub fn forget(&self, name: &ProfileName) {
        let _ = std::fs::remove_file(self.directory.join(name.as_str()));
    }

    pub fn leftover(&self) -> Vec<ProfileName> {
        let Ok(entries) = std::fs::read_dir(&self.directory) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter_map(|entry| ProfileName::parse(entry.file_name().to_str()?))
            .collect()
    }
}

#[cfg(test)]
mod tests;
