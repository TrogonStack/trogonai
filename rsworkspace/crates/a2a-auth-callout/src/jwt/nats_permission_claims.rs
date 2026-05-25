use serde::{Deserialize, Serialize};

use crate::permissions::IssuedPermissions;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NatsSubjectPermission {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
}

impl NatsSubjectPermission {
    pub fn allow_only(patterns: impl IntoIterator<Item = String>) -> Self {
        Self {
            allow: patterns.into_iter().collect(),
            deny: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NatsPermissionClaims {
    #[serde(rename = "pub")]
    pub publish: NatsSubjectPermission,
    #[serde(rename = "sub")]
    pub subscribe: NatsSubjectPermission,
}

impl From<&IssuedPermissions> for NatsPermissionClaims {
    fn from(perms: &IssuedPermissions) -> Self {
        Self {
            publish: NatsSubjectPermission::allow_only(
                perms
                    .publish_allow
                    .iter()
                    .map(|p| p.as_str().to_owned()),
            ),
            subscribe: NatsSubjectPermission::allow_only(
                perms
                    .subscribe_allow
                    .iter()
                    .map(|p| p.as_str().to_owned()),
            ),
        }
    }
}
