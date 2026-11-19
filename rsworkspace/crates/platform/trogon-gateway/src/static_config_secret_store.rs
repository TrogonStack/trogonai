use std::env::VarError;
use std::fmt;

use trogon_credential_domain::{CredentialKind, CredentialScope, PublicCredentialId};
use trogon_secret_store::SecretMaterial;
use trogon_std::env::ReadEnv;
use trogon_std::{EmptySecretError, SecretString};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StaticConfigSecretInput {
    Literal(String),
    Env { name: String },
}

impl StaticConfigSecretInput {
    pub(crate) fn resolve_plaintext(self, env: &impl ReadEnv) -> Result<SecretString, StaticConfigSecretStoreError> {
        let raw_value = match self {
            Self::Literal(value) => value,
            Self::Env { name } => {
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
        SecretString::new(raw_value).map_err(StaticConfigSecretStoreError::EmptySecretError)
    }
}

/// A secret read from static configuration.
///
/// It never enters the credential event stream, so it has no recorded identity
/// and is named by its public id alone.
#[derive(Clone)]
pub struct StaticConfigSecret {
    credential: PublicCredentialId,
    material: SecretMaterial,
}

impl StaticConfigSecret {
    fn new(credential: PublicCredentialId, material: SecretMaterial) -> Self {
        Self { credential, material }
    }

    pub fn into_plaintext(self) -> Result<SecretString, StaticConfigSecretStoreError> {
        match self.material {
            SecretMaterial::Plaintext(value) => Ok(value),
            SecretMaterial::Verifier(_) => Err(StaticConfigSecretStoreError::VerifierOnly {
                credential: self.credential,
            }),
        }
    }
}

impl fmt::Debug for StaticConfigSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaticConfigSecret")
            .field("credential", &self.credential)
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
        let material = SecretMaterial::plaintext(input.resolve_plaintext(env)?);
        let credential = PublicCredentialId::new(scope.owner_id(), &scope.scope_key(), kind);

        Ok(StaticConfigSecret::new(credential, material))
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
    VerifierOnly { credential: PublicCredentialId },
}

#[cfg(test)]
mod tests;
