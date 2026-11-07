use std::fmt;

/// Which adapter holds a credential's material.
///
/// Adapter-private knowledge of the aggregate, recovered from its
/// `StorageLocator`. It is not part of any contract, because ADR#0023 keeps
/// storage backends out of them.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum StorageBackend {
    InMemory,
    OpenBao,
    StaticConfig,
}

impl StorageBackend {
    pub(super) fn from_locator(locator: &str) -> Result<Self, StorageBackendError> {
        let Some((prefix, _)) = locator.split_once(':') else {
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
    #[error("storage locator has no backend prefix")]
    MissingPrefix,
    #[error("storage locator has unknown backend prefix '{0}'")]
    UnknownPrefix(String),
}

#[cfg(test)]
mod tests;
