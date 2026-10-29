use std::fmt;

use super::CredentialId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageBackend {
    InMemory,
    OpenBao,
    StaticConfig,
}

impl StorageBackend {
    pub fn from_credential_id(id: &CredentialId) -> Result<Self, StorageBackendError> {
        let Some((prefix, _)) = id.as_str().split_once(':') else {
            return Err(StorageBackendError::MissingPrefix);
        };
        match prefix {
            "memory" => Ok(Self::InMemory),
            "openbao" => Ok(Self::OpenBao),
            "static-config" => Ok(Self::StaticConfig),
            _ => Err(StorageBackendError::UnknownPrefix(prefix.to_string())),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::InMemory => "in_memory",
            Self::OpenBao => "openbao",
            Self::StaticConfig => "static_config",
        }
    }
}

impl fmt::Display for StorageBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum StorageBackendError {
    #[error("credential id has no storage prefix")]
    MissingPrefix,
    #[error("credential id has unknown storage prefix '{0}'")]
    UnknownPrefix(String),
}

#[cfg(test)]
mod tests;
