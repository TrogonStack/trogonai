use std::fmt;

use super::CredentialKey;
use super::constants::{COLLECTION_SEGMENT, CREDENTIAL_PATH_SEGMENTS, ROOT_SEGMENT};

/// Where a credential's material lives in the managed subtree.
///
/// The path comes from the credential's key alone, so it names neither the scope
/// nor the kind, and a credential keeps its path for as long as it keeps its id.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CredentialPath {
    key: CredentialKey,
}

impl CredentialPath {
    pub fn new(key: CredentialKey) -> Self {
        Self { key }
    }

    pub fn segments(&self) -> [String; CREDENTIAL_PATH_SEGMENTS] {
        [
            ROOT_SEGMENT.to_string(),
            self.key.owner_id().as_str().to_string(),
            COLLECTION_SEGMENT.to_string(),
            self.key.credential_id().to_string(),
        ]
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
