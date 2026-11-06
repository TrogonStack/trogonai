use std::path::PathBuf;

use trogon_policy_eval::RuleNameError;

use crate::rule_set_error::RuleSetError;

/// Why a bundle directory is not loaded. The set in force is left as it was.
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    /// The directory or a file in it could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// What was being read.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A rule file is larger than the source limit.
    #[error("rule file {path} is larger than {limit} bytes")]
    FileTooLarge {
        /// The file.
        path: PathBuf,
        /// The source limit.
        limit: usize,
    },
    /// A file name cannot be a rule name.
    #[error("rule file {path} has a name that cannot name a rule: {source}")]
    InvalidName {
        /// The file.
        path: PathBuf,
        /// Why the name was refused.
        source: RuleNameError,
    },
    /// The files read, but together they are not a valid set.
    #[error(transparent)]
    RuleSet(#[from] RuleSetError),
}
