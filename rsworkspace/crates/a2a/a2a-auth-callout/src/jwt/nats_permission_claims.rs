use serde::{Deserialize, Serialize};

use crate::constants::FULL_WILDCARD_SUBJECT;
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
        let allow: Vec<String> = patterns.into_iter().collect();
        let deny = if allow.is_empty() {
            vec![FULL_WILDCARD_SUBJECT.to_owned()]
        } else {
            Vec::new()
        };
        Self { allow, deny }
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
            publish: NatsSubjectPermission::allow_only(perms.publish_allow.iter().map(|p| p.as_str().to_owned())),
            subscribe: NatsSubjectPermission::allow_only(perms.subscribe_allow.iter().map(|p| p.as_str().to_owned())),
        }
    }
}

#[cfg(test)]
mod tests;
