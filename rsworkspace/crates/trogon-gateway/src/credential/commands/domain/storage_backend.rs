use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageBackend {
    InMemory,
    OpenBao,
    StaticConfig,
}

impl StorageBackend {
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
