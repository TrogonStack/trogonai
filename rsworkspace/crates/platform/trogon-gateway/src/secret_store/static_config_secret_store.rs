use std::env::VarError;
use std::fmt;

use trogon_std::env::ReadEnv;
use trogon_std::{EmptySecretError, SecretString};

use super::{SecretMaterial, fingerprint};
use crate::credential::commands::domain::{
    CredentialFingerprintError, CredentialId, CredentialKind, CredentialMetadata, CredentialRef, CredentialScope,
    CredentialStatus, CredentialVersion, StorageLocator, StorageLocatorError,
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
        env: &impl ReadEnv,
    ) -> Result<StaticConfigSecret, StaticConfigSecretStoreError> {
        let raw_value = match input {
            StaticConfigSecretInput::Literal(value) => value,
            StaticConfigSecretInput::Env { name } => {
                let name = name.trim();
                if name.is_empty() {
                    return Err(StaticConfigSecretStoreError::EmptyEnvName);
                }
                env.var(name).map_err(|error| match error {
                    VarError::NotPresent => StaticConfigSecretStoreError::MissingEnv { name: name.to_string() },
                    VarError::NotUnicode(_) => {
                        StaticConfigSecretStoreError::InvalidUnicodeEnv { name: name.to_string() }
                    }
                })?
            }
        };
        let material = SecretString::new(raw_value)
            .map(SecretMaterial::plaintext)
            .map_err(StaticConfigSecretStoreError::EmptySecretError)?;
        let reference = Self::credential_ref(&scope, kind);

        Ok(StaticConfigSecret::new(reference, material))
    }

    pub fn metadata(secret: &StaticConfigSecret) -> Result<CredentialMetadata, StaticConfigSecretStoreError> {
        let fingerprint = fingerprint::digest("static-config", secret.reference())
            .map_err(StaticConfigSecretStoreError::InvalidFingerprint)?;

        let locator = StorageLocator::static_config(secret.reference())
            .map_err(StaticConfigSecretStoreError::InvalidStorageLocator)?;

        Ok(CredentialMetadata::new(
            secret.reference().clone(),
            CredentialStatus::Active,
            locator,
            fingerprint,
        ))
    }

    fn credential_ref(scope: &CredentialScope, kind: CredentialKind) -> CredentialRef {
        CredentialRef::new(
            CredentialId::derive(scope, kind),
            CredentialVersion::initial(),
            scope,
            kind,
        )
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
    EmptySecretError(#[source] EmptySecretError),
    #[error("credential is verifier-only: {credential}")]
    VerifierOnly { credential: CredentialRef },
    #[error("invalid storage locator: {0}")]
    InvalidStorageLocator(#[source] StorageLocatorError),
    #[error("invalid credential fingerprint: {0}")]
    InvalidFingerprint(#[source] CredentialFingerprintError),
}

#[cfg(test)]
mod tests;
