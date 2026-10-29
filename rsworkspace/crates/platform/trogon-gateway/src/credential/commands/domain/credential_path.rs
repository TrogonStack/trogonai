use std::fmt;

use super::constants::{COLLECTION_SEGMENT, CREDENTIAL_PATH_SEGMENTS, ROOT_SEGMENT};
use super::{CredentialId, CredentialOwnerId, CredentialRef};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CredentialPath {
    owner_id: CredentialOwnerId,
    credential_id: CredentialId,
}

impl CredentialPath {
    pub fn new(owner_id: CredentialOwnerId, credential_id: CredentialId) -> Self {
        Self {
            owner_id,
            credential_id,
        }
    }

    pub fn segments(&self) -> [&str; CREDENTIAL_PATH_SEGMENTS] {
        [
            ROOT_SEGMENT,
            self.owner_id.as_str(),
            COLLECTION_SEGMENT,
            self.credential_id.as_str(),
        ]
    }
}

impl From<&CredentialRef> for CredentialPath {
    fn from(credential: &CredentialRef) -> Self {
        Self::new(credential.owner_id().clone(), credential.id().clone())
    }
}

impl fmt::Display for CredentialPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [root, owner_id, collection, credential_id] = self.segments();
        write!(f, "{root}/{owner_id}/{collection}/{credential_id}")
    }
}

#[cfg(test)]
mod tests;
