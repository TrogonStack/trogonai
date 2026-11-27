use serde::{Deserialize, Serialize};

use crate::{FilesystemEnforcement, GitConfigAccess, MandatoryDenySearchDepth};

/// What a lease relaxes in the mandatory filesystem protections, on every platform.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct FilesystemOptions {
    /// Leaves `.git/config` writable where the rest of the git directory is protected. Hooks stay protected.
    #[serde(default)]
    pub allow_git_config: GitConfigAccess,
    /// Applies no filesystem rules at all when disabled.
    #[serde(default)]
    pub disabled: FilesystemEnforcement,
    /// Limits how deep the Linux scan looks for protected names, counted from the working directory. Absent, the
    /// depth is [`MandatoryDenySearchDepth::DEFAULT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mandatory_deny_search_depth: Option<MandatoryDenySearchDepth>,
}

impl FilesystemOptions {
    /// Reports whether nothing is relaxed, so the wire can leave the field out.
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}
