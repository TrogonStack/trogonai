use std::fmt;

use trogon_service_config::SecretReference;
use trogon_std::env::ReadEnv;

use crate::constants::SIGNING_KEY_SOURCE_VAR;
use crate::error::AuthCalloutError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigningKeyCustody {
    Env,
    File,
}

impl SigningKeyCustody {
    pub fn from_env(env: &impl ReadEnv) -> Result<Self, AuthCalloutError> {
        let Ok(source) = env.var(SIGNING_KEY_SOURCE_VAR) else {
            return Ok(Self::Env);
        };
        match source.as_str() {
            "env" => Ok(Self::Env),
            "file" => Ok(Self::File),
            "vault" => Err(AuthCalloutError::VaultNotConfigured),
            other => Err(AuthCalloutError::UnknownSigningKeySource(other.to_owned())),
        }
    }

    pub fn holds(self, reference: &SecretReference) -> bool {
        matches!(
            (self, reference),
            (Self::Env, SecretReference::Env(_)) | (Self::File, SecretReference::File(_))
        )
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Env => "env",
            Self::File => "file",
        }
    }
}

impl fmt::Display for SigningKeyCustody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
