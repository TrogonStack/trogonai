use std::fmt;

use serde::{Deserialize, Serialize};

use crate::GitProvider;

/// The repository a sandbox's workspace was cloned from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoRef {
    pub provider: GitProvider,
    pub owner: String,
    pub name: String,
}

impl fmt::Display for RepoRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}/{}", self.provider, self.owner, self.name)
    }
}
