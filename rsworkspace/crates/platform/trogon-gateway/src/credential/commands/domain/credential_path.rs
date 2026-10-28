use std::fmt;

use super::{CredentialId, CredentialOwnerId, CredentialRef};

const ROOT_SEGMENT: &str = "trogonai";
const COLLECTION_SEGMENT: &str = "credentials";

pub const CREDENTIAL_PATH_SEGMENTS: usize = 4;

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
mod tests {
    use super::*;
    use crate::credential::commands::domain::{CredentialKind, CredentialScope, CredentialVersion, SourceKind};
    use crate::source_integration_id::SourceIntegrationId;

    fn owner() -> CredentialOwnerId {
        CredentialOwnerId::new("tenant-1").unwrap()
    }

    #[test]
    fn renders_the_managed_subtree_layout() {
        let path = CredentialPath::new(
            owner(),
            CredentialId::new("openbao:tenant-1:github:webhook_secret").unwrap(),
        );

        assert_eq!(
            path.to_string(),
            "trogonai/tenant-1/credentials/openbao:tenant-1:github:webhook_secret"
        );
    }

    #[test]
    fn segments_are_unjoined_so_a_caller_can_encode_each_one() {
        let path = CredentialPath::new(
            owner(),
            CredentialId::new("openbao:tenant-1:github/acme:webhook_secret").unwrap(),
        );

        assert_eq!(
            path.segments(),
            [
                "trogonai",
                "tenant-1",
                "credentials",
                "openbao:tenant-1:github/acme:webhook_secret",
            ]
        );
    }

    #[test]
    fn a_credential_ref_locates_the_same_path_for_every_version() {
        let scope =
            CredentialScope::integration(owner(), SourceKind::GitHub, SourceIntegrationId::new("acme").unwrap());
        let id = CredentialId::new("openbao:tenant-1:github/acme:webhook_secret").unwrap();
        let first = CredentialRef::new(
            id.clone(),
            CredentialVersion::initial(),
            &scope,
            CredentialKind::WebhookSecret,
        );
        let rotated = first.next_version();

        assert_eq!(CredentialPath::from(&first), CredentialPath::from(&rotated));
    }
}
