use serde::{Deserialize, Serialize};

use crate::FsPathEntry;

/// The JSON shape of an [`FsPolicy`](crate::FsPolicy) before its list rules are checked.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FsPolicyWire {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) deny_read: Vec<FsPathEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) credential_deny_read: Vec<FsPathEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) allow_read_within_deny: Vec<FsPathEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) own_allow_within_deny: Vec<FsPathEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) allow_write: Vec<FsPathEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) deny_write: Vec<FsPathEntry>,
}
