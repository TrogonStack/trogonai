use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use trogon_policy_eval::RuleName;

use crate::bundle_error::BundleError;
use crate::bundle_handle::BundleHandle;
use crate::cel_limits::CelLimits;
use crate::constants::{MAX_SOURCE_BYTES, RULE_FILE_EXTENSION};
use crate::rule_set::RuleSet;

/// Reads a directory of rule files and compiles it into a [`RuleSet`].
///
/// Each file named `<rule name>.cel` holds one rule. Other files and
/// subdirectories are ignored, so a bundle can carry a README. The loader is
/// the only code in this crate that touches the filesystem, and it is meant to
/// run on a timer or a change notification, never inside a request:
/// [`CelEngine`](crate::CelEngine) reads the [`BundleHandle`] and nothing else.
/// A failed reload leaves the set in force untouched, so a bad edit cannot take
/// the request rules away or replace them with half of a bundle.
#[derive(Debug, Clone)]
pub struct BundleLoader {
    directory: PathBuf,
    limits: CelLimits,
}

impl BundleLoader {
    /// Prepares to load the rules in this directory under these limits. The
    /// directory is not read until [`Self::load`].
    pub fn new(directory: impl Into<PathBuf>, limits: CelLimits) -> Self {
        Self {
            directory: directory.into(),
            limits,
        }
    }

    /// Reads and compiles every rule file.
    pub fn load(&self) -> Result<RuleSet, BundleError> {
        let mut sources = Vec::new();
        for path in self.rule_files()? {
            let name = rule_name_of(&path)?;
            sources.push((name, read_source(&path)?));
        }
        Ok(RuleSet::compile(sources, &self.limits)?)
    }

    /// Loads the bundle and, only if all of it is valid, makes it the set in
    /// force. Returns how many rules are now in force.
    pub fn reload_into(&self, handle: &BundleHandle) -> Result<usize, BundleError> {
        let rules = self.load()?;
        let count = rules.len();
        handle.replace(rules);
        Ok(count)
    }

    fn rule_files(&self) -> Result<Vec<PathBuf>, BundleError> {
        let io = |path: &Path| {
            let path = path.to_path_buf();
            move |source| BundleError::Io { path, source }
        };
        let mut files = Vec::new();
        for entry in fs::read_dir(&self.directory).map_err(io(&self.directory))? {
            let entry = entry.map_err(io(&self.directory))?;
            let path = entry.path();
            let is_file = entry.file_type().map_err(io(&path))?.is_file();
            if is_file
                && path
                    .extension()
                    .is_some_and(|extension| extension == RULE_FILE_EXTENSION)
            {
                files.push(path);
            }
        }
        Ok(files)
    }
}

fn rule_name_of(path: &Path) -> Result<RuleName, BundleError> {
    let stem = path.file_stem().map(|stem| stem.to_string_lossy().into_owned());
    RuleName::new(stem.unwrap_or_default()).map_err(|source| BundleError::InvalidName {
        path: path.to_path_buf(),
        source,
    })
}

fn read_source(path: &Path) -> Result<String, BundleError> {
    let io = |source| BundleError::Io {
        path: path.to_path_buf(),
        source,
    };
    let limit = u64::try_from(MAX_SOURCE_BYTES).unwrap_or(u64::MAX);
    let mut source = String::new();
    File::open(path)
        .map_err(io)?
        .take(limit.saturating_add(1))
        .read_to_string(&mut source)
        .map_err(io)?;
    if source.len() > MAX_SOURCE_BYTES {
        return Err(BundleError::FileTooLarge {
            path: path.to_path_buf(),
            limit: MAX_SOURCE_BYTES,
        });
    }
    Ok(source)
}
