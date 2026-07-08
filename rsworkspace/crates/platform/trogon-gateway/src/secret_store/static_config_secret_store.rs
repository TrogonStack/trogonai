use std::env;
use std::fmt;

use trogon_std::{EmptySecret, SecretString};

use super::SecretMaterial;
use crate::credential::commands::domain::{
    CredentialFingerprintError, CredentialId, CredentialIdError, CredentialKind, CredentialMetadata, CredentialRef,
    CredentialScope, CredentialStatus, CredentialVersion, StorageBackend,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StaticConfigSecretInput {
    Literal(String),
    Env { name: String },
}

#[derive(Clone)]
pub struct StaticConfigSecret {
    reference: CredentialRef,
    material: SecretMaterial,
}

impl StaticConfigSecret {
    fn new(reference: CredentialRef, material: SecretMaterial) -> Self {
        Self { reference, material }
    }

    pub fn reference(&self) -> &CredentialRef {
        &self.reference
    }

    pub fn into_plaintext(self) -> Result<SecretString, StaticConfigSecretStoreError> {
        match self.material {
            SecretMaterial::Plaintext(value) => Ok(value),
            SecretMaterial::Verifier(_) => Err(StaticConfigSecretStoreError::VerifierOnly {
                credential: self.reference,
            }),
        }
    }
}

impl fmt::Debug for StaticConfigSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaticConfigSecret")
            .field("reference", &self.reference)
            .field("material", &"***")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StaticConfigSecretStore;

impl StaticConfigSecretStore {
    pub fn resolve(
        scope: CredentialScope,
        kind: CredentialKind,
        input: StaticConfigSecretInput,
    ) -> Result<StaticConfigSecret, StaticConfigSecretStoreError> {
        let raw_value = match input {
            StaticConfigSecretInput::Literal(value) => value,
            StaticConfigSecretInput::Env { name } => {
                let name = name.trim();
                if name.is_empty() {
                    return Err(StaticConfigSecretStoreError::EmptyEnvName);
                }
                env::var(name).map_err(|error| match error {
                    env::VarError::NotPresent => StaticConfigSecretStoreError::MissingEnv { name: name.to_string() },
                    env::VarError::NotUnicode(_) => {
                        StaticConfigSecretStoreError::InvalidUnicodeEnv { name: name.to_string() }
                    }
                })?
            }
        };
        let material = SecretString::new(raw_value)
            .map(SecretMaterial::plaintext)
            .map_err(StaticConfigSecretStoreError::EmptySecret)?;
        let reference = Self::credential_ref(&scope, kind)?;

        Ok(StaticConfigSecret::new(reference, material))
    }

    pub fn metadata(secret: &StaticConfigSecret) -> Result<CredentialMetadata, StaticConfigSecretStoreError> {
        let fingerprint = crate::credential::commands::domain::CredentialFingerprint::new(format!(
            "static-config:{}",
            secret.reference()
        ))
        .map_err(StaticConfigSecretStoreError::InvalidFingerprint)?;

        Ok(CredentialMetadata::new(
            secret.reference().clone(),
            CredentialStatus::Active,
            StorageBackend::StaticConfig,
            fingerprint,
        ))
    }

    fn credential_ref(
        scope: &CredentialScope,
        kind: CredentialKind,
    ) -> Result<CredentialRef, StaticConfigSecretStoreError> {
        let id = CredentialId::new(format!("static-config:{}:{}", scope.scope_key(), kind.as_str()))
            .map_err(StaticConfigSecretStoreError::InvalidCredentialId)?;

        Ok(CredentialRef::new(id, CredentialVersion::initial(), scope, kind))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StaticConfigSecretStoreError {
    #[error("env var name must not be empty")]
    EmptyEnvName,
    #[error("env var '{name}' is not set")]
    MissingEnv { name: String },
    #[error("env var '{name}' is not valid unicode")]
    InvalidUnicodeEnv { name: String },
    #[error("{0}")]
    EmptySecret(#[source] EmptySecret),
    #[error("credential is verifier-only: {credential}")]
    VerifierOnly { credential: CredentialRef },
    #[error("invalid credential id: {0}")]
    InvalidCredentialId(#[source] CredentialIdError),
    #[error("invalid credential fingerprint: {0}")]
    InvalidFingerprint(#[source] CredentialFingerprintError),
}

#[cfg(test)]
mod tests {
    use crate::credential::commands::domain::{CredentialOwnerId, SourceKind};
    use crate::source_integration_id::SourceIntegrationId;

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
        );

        assert!(matches!(result, Err(StaticConfigSecretStoreError::EmptySecret(_))));
    }
}
