use std::fmt;

use super::{CredentialKind, CredentialOwnerId};

/// Identifier for a credential as seen outside the gateway.
///
/// ADR#0023 requires that storage-provider artifacts never appear in wire
/// contracts, so this is composed from validated domain identifiers rather than
/// derived from `CredentialId`, whose value names the storage backend that
/// happens to hold the material.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PublicCredentialId(String);

impl PublicCredentialId {
    pub fn new(owner_id: &CredentialOwnerId, scope_key: &str, kind: CredentialKind) -> Self {
        Self(format!("{}:{}:{}", owner_id.as_str(), scope_key, kind.as_str()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for PublicCredentialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
