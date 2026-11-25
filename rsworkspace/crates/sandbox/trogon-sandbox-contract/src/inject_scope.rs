use serde::{Deserialize, Serialize};

use crate::{InjectMethodSet, InjectPathPrefix, InjectScopeVersion};

/// Where on a destination one credential binding may be injected.
///
/// Every field is optional and defaults, so a binding written without a scope is unrestricted and keeps the original
/// port meaning, where an omitted port is any port. It serializes flat inside its binding.
#[derive(Debug, Clone, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct InjectScope {
    /// Only requests at or below this path receive the credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<InjectPathPrefix>,
    /// Only requests with one of these methods receive the credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub methods: Option<InjectMethodSet>,
    /// `1` (default): a host pattern without a port spans every port. `2`: it means port 443.
    #[serde(default, skip_serializing_if = "InjectScopeVersion::is_default")]
    pub scope_version: InjectScopeVersion,
}

impl InjectScope {
    /// Reports whether no path or method restriction applies, so the host decides alone.
    pub fn is_request_independent(&self) -> bool {
        self.path_prefix.is_none() && self.methods.is_none()
    }

    /// Reports whether this is the unrestricted default scope.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}
