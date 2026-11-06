use std::fmt;

use super::{CredentialId, CredentialOwnerId, CredentialRef};

/// Identity of a credential aggregate's event stream.
///
/// The opaque `CredentialId` carries no hierarchy, so the project that owns the
/// credential travels beside it. The store routes on the owner to give each
/// project its own ADR#0027 subject scope, which is what keeps an aggregate
/// from being addressable outside its project.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CredentialStreamId {
    owner_id: CredentialOwnerId,
    credential_id: CredentialId,
}

impl CredentialStreamId {
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

impl From<&CredentialRef> for CredentialStreamId {
    fn from(credential: &CredentialRef) -> Self {
        Self::new(credential.owner_id().clone(), credential.id().clone())
    }
}

impl fmt::Display for CredentialStreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner_id, self.credential_id)
    }
}

#[cfg(test)]
mod tests;
