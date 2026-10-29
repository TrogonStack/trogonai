use crate::credential::commands::domain::{CredentialOwnerId, SourceKind};
use crate::source_integration_id::SourceIntegrationId;
use trogon_std::env::{InMemoryEnv, ReadEnv};

use super::*;
use crate::secret_store::SecretVerifier;

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
fn metadata_fingerprint_stays_bounded_for_the_longest_integration_id() {
    let scope = CredentialScope::integration(
        CredentialOwnerId::static_config(),
        SourceKind::GooglePubSub,
        SourceIntegrationId::new("a".repeat(64)).unwrap(),
    );
    let secret = StaticConfigSecretStore::resolve(
        scope,
        CredentialKind::ServiceAccountCredentials,
        StaticConfigSecretInput::Literal("service-account-json".to_string()),
        &InMemoryEnv::new(),
    )
    .unwrap();

    let metadata = StaticConfigSecretStore::metadata(&secret).unwrap();

    assert_eq!(metadata.fingerprint().as_str().len(), "static-config:".len() + 64);
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

fn discord_scope() -> CredentialScope {
    CredentialScope::source(CredentialOwnerId::static_config(), SourceKind::Discord)
}

#[test]
fn debug_redacts_the_material() {
    let secret = StaticConfigSecretStore::resolve(
        discord_scope(),
        CredentialKind::BotToken,
        StaticConfigSecretInput::Literal("Bot super-secret".to_string()),
        &InMemoryEnv::new(),
    )
    .unwrap();

    let debug = format!("{secret:?}");

    assert!(debug.contains("StaticConfigSecret"));
    assert!(debug.contains("***"));
    assert!(!debug.contains("super-secret"));
}

#[test]
fn verifier_only_material_cannot_be_read_as_plaintext() {
    let reference = StaticConfigSecretStore::credential_ref(&discord_scope(), CredentialKind::BotToken).unwrap();
    let secret = StaticConfigSecret::new(
        reference.clone(),
        SecretMaterial::Verifier(SecretVerifier::new("verifier").unwrap()),
    );

    let error = secret.into_plaintext().unwrap_err();

    assert!(matches!(
        error,
        StaticConfigSecretStoreError::VerifierOnly { credential } if credential == reference
    ));
}

#[test]
fn env_input_resolves_the_named_variable() {
    let env = InMemoryEnv::new();
    env.set("DISCORD_BOT_TOKEN", "Bot from-env");

    let secret = StaticConfigSecretStore::resolve(
        discord_scope(),
        CredentialKind::BotToken,
        StaticConfigSecretInput::Env {
            name: " DISCORD_BOT_TOKEN ".to_string(),
        },
        &env,
    )
    .unwrap();

    assert_eq!(secret.into_plaintext().unwrap().as_str(), "Bot from-env");
}

#[test]
fn env_input_with_blank_name_is_rejected() {
    let result = StaticConfigSecretStore::resolve(
        discord_scope(),
        CredentialKind::BotToken,
        StaticConfigSecretInput::Env { name: "  ".to_string() },
        &InMemoryEnv::new(),
    );

    assert!(matches!(result, Err(StaticConfigSecretStoreError::EmptyEnvName)));
}

#[test]
fn env_input_naming_an_unset_variable_is_rejected() {
    let result = StaticConfigSecretStore::resolve(
        discord_scope(),
        CredentialKind::BotToken,
        StaticConfigSecretInput::Env {
            name: "UNSET_TOKEN".to_string(),
        },
        &InMemoryEnv::new(),
    );

    assert!(matches!(
        result,
        Err(StaticConfigSecretStoreError::MissingEnv { name }) if name == "UNSET_TOKEN"
    ));
}

struct NonUnicodeEnv;

impl ReadEnv for NonUnicodeEnv {
    fn var(&self, _key: &str) -> Result<String, VarError> {
        Err(VarError::NotUnicode(std::ffi::OsString::new()))
    }
}

#[test]
fn env_input_with_non_unicode_value_is_rejected() {
    let result = StaticConfigSecretStore::resolve(
        discord_scope(),
        CredentialKind::BotToken,
        StaticConfigSecretInput::Env {
            name: "BINARY_TOKEN".to_string(),
        },
        &NonUnicodeEnv,
    );

    assert!(matches!(
        result,
        Err(StaticConfigSecretStoreError::InvalidUnicodeEnv { name }) if name == "BINARY_TOKEN"
    ));
}
