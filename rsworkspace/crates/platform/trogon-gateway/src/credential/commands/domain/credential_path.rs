use std::fmt;

use super::constants::{COLLECTION_SEGMENT, CREDENTIAL_PATH_SEGMENTS, ROOT_SEGMENT};
use super::{CredentialOwnerId, StorageLocator};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CredentialPath {
    owner_id: CredentialOwnerId,
    storage_locator: StorageLocator,
}

impl CredentialPath {
    pub fn new(owner_id: CredentialOwnerId, storage_locator: StorageLocator) -> Self {
        Self {
            owner_id,
            storage_locator,
        }
    }

    pub fn segments(&self) -> [&str; CREDENTIAL_PATH_SEGMENTS] {
        [
            ROOT_SEGMENT,
            self.owner_id.as_str(),
            COLLECTION_SEGMENT,
            self.storage_locator.as_str(),
        ]
    }
}

impl fmt::Display for CredentialPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [root, owner_id, collection, locator] = self.segments();
        write!(f, "{root}/{owner_id}/{collection}/{locator}")
    }
}

#[cfg(test)]
mod tests;
