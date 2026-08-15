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
mod tests {
    use super::*;
    use crate::credential::commands::domain::CredentialId;
    use crate::credential::commands::domain::{CredentialRef, CredentialScope, CredentialVersion, SourceKind};
    use crate::secret_store::openbao_secret_store::openbao_credential_id;
    use crate::source_integration_id::SourceIntegrationId;

    fn owner() -> CredentialOwnerId {
        CredentialOwnerId::new("tenant-1").unwrap()
    }

    #[test]
    fn composes_owner_scope_and_kind() {
        let id = PublicCredentialId::new(&owner(), "github/primary", CredentialKind::WebhookSecret);

        assert_eq!(id.as_str(), "tenant-1:github/primary:webhook_secret");
    }

    #[test]
    fn omits_the_storage_backend_named_by_the_internal_id() {
        let scope = CredentialScope::source(owner(), SourceKind::Discord);
        let internal = openbao_credential_id(&scope, CredentialKind::BotToken).unwrap();
        let reference = CredentialRef::new(
            internal.clone(),
            CredentialVersion::initial(),
            &scope,
            CredentialKind::BotToken,
        );

        assert!(internal.as_str().starts_with("openbao:"));
        assert_eq!(reference.public_id().as_str(), "tenant-1:discord:bot_token");
    }

    #[test]
    fn is_stable_across_storage_backends() {
        let scope = CredentialScope::integration(
            owner(),
            SourceKind::GitHub,
            SourceIntegrationId::new("primary").unwrap(),
        );
        let openbao = CredentialRef::new(
            openbao_credential_id(&scope, CredentialKind::WebhookSecret).unwrap(),
            CredentialVersion::initial(),
            &scope,
            CredentialKind::WebhookSecret,
        );
        let static_config = CredentialRef::new(
            CredentialId::new("static-config:github/primary:webhook_secret").unwrap(),
            CredentialVersion::initial(),
            &scope,
            CredentialKind::WebhookSecret,
        );

        assert_eq!(openbao.public_id(), static_config.public_id());
    }
}
