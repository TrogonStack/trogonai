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
mod tests {
    use ard_catalog::{ArdIdentifier, ArdStorageKey};
    use trogon_nats::NatsToken;

    use super::{CatalogEventSubject, CatalogSubjectKind};

    #[test]
    fn subject_uses_storage_key_not_raw_identifier() {
        let identifier = ArdIdentifier::new("urn:air:example.com:agent:assistant").unwrap();
        let storage_key = ArdStorageKey::from_identifier(&identifier);
        let subject = CatalogEventSubject::new(CatalogSubjectKind::Upserted, &storage_key).unwrap();

        assert!(!subject.to_string().contains(identifier.as_str()));
        assert!(subject.to_string().ends_with(storage_key.as_str()));
        assert!(NatsToken::new(storage_key.as_str()).is_ok());
    }
}
