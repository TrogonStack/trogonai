use std::fmt;

use super::{CredentialId, CredentialOwnerId};

/// Names a credential aggregate: its id together with the project that owns it.
///
/// A credential id is unique only within its project, so the owner travels beside
/// it. The store routes on the owner to give each project its own ADR#0027 subject
/// scope, which is what keeps an aggregate from being addressable outside its
/// project. Every event after the write request names its credential by this key.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CredentialKey {
    owner_id: CredentialOwnerId,
    credential_id: CredentialId,
}

impl CredentialKey {
    pub fn new(owner_id: CredentialOwnerId, credential_id: CredentialId) -> Self {
        Self {
            owner_id,
            credential_id,
        }
    }

    pub fn owner_id(&self) -> &CredentialOwnerId {
        &self.owner_id
    }

    pub fn credential_id(&self) -> &CredentialId {
        &self.credential_id
    }
}

impl fmt::Display for CredentialKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner_id, self.credential_id)
    }
}

#[cfg(test)]
mod tests;
