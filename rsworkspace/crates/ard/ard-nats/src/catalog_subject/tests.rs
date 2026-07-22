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
