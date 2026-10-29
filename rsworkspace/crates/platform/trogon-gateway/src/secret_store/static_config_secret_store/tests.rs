use crate::credential::commands::domain::{CredentialOwnerId, SourceKind};
use crate::source_integration_id::SourceIntegrationId;
use trogon_std::env::InMemoryEnv;

use super::*;

#[test]
fn resolves_literal_to_static_reference() {
    let scope = CredentialScope::integration(
        CredentialOwnerId::static_config(),
        SourceKind::GitHub,
        SourceIntegrationId::new("primary").unwrap(),
    );
    let secret = StaticConfigSecretStore::resolve(
        scope,
        CredentialKind::WebhookSecret,
        StaticConfigSecretInput::Literal("gh-secret".to_string()),
        &InMemoryEnv::new(),
    )
    .unwrap();

    assert_eq!(
        secret.reference().to_string(),
        "static-config:github/primary:webhook_secret@1"
    );
    assert_eq!(secret.into_plaintext().unwrap().as_str(), "gh-secret");
}

#[test]
fn empty_literal_is_invalid_at_static_write_boundary() {
    let result = StaticConfigSecretStore::resolve(
        CredentialScope::source(CredentialOwnerId::static_config(), SourceKind::Discord),
        CredentialKind::BotToken,
        StaticConfigSecretInput::Literal(String::new()),
        &InMemoryEnv::new(),
    );

    assert!(matches!(result, Err(StaticConfigSecretStoreError::EmptySecretError(_))));
}
