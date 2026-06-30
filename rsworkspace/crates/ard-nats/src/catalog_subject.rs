use ard_catalog::ArdStorageKey;
use trogon_nats::{NatsToken, SubjectTokenViolation};

pub const CATALOG_KV_BUCKET: &str = "ARD_CATALOG";
pub const CATALOG_EVENT_STREAM: &str = "ARD_CATALOG_EVENTS";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogSubjectKind {
    Upserted,
    Deleted,
    Validated,
    Indexed,
}

impl CatalogSubjectKind {
    fn as_token(self) -> &'static str {
        match self {
            Self::Upserted => "upserted",
            Self::Deleted => "deleted",
            Self::Validated => "validated",
            Self::Indexed => "indexed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEventSubject {
    storage_key: NatsToken,
    kind: CatalogSubjectKind,
}

impl CatalogEventSubject {
    pub fn new(kind: CatalogSubjectKind, storage_key: &ArdStorageKey) -> Result<Self, SubjectTokenViolation> {
        Ok(Self {
            storage_key: NatsToken::new(storage_key.as_str())?,
            kind,
        })
    }
}

impl std::fmt::Display for CatalogEventSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ard.catalog.entry.{}.{}", self.kind.as_token(), self.storage_key)
    }
}

#[cfg(test)]
mod tests;
