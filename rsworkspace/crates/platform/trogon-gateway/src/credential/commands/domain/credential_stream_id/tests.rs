use super::*;
use crate::credential::commands::domain::{CredentialKind, CredentialScope, CredentialVersion, SourceKind};

#[test]
fn pairs_the_owner_with_the_opaque_id() {
    let owner = CredentialOwnerId::new("tenant-1").unwrap();
    let scope = CredentialScope::source(owner.clone(), SourceKind::GitHub);
    let id = CredentialId::derive(&scope, CredentialKind::WebhookSecret);
    let reference = CredentialRef::new(
        id.clone(),
        CredentialVersion::initial(),
        &scope,
        CredentialKind::WebhookSecret,
    );

    let stream_id = CredentialStreamId::from(&reference);

    assert_eq!(stream_id, CredentialStreamId::new(owner.clone(), id.clone()));
    assert_eq!(stream_id.owner_id(), &owner);
    assert_eq!(stream_id.credential_id(), &id);
    assert_eq!(stream_id.to_string(), format!("tenant-1/{id}"));
}

#[test]
fn the_same_id_under_another_owner_is_another_stream() {
    let id = CredentialId::new("ab6f9f8b4ffc5bd9b36bf3b0fa5e3da4").unwrap();

    assert_ne!(
        CredentialStreamId::new(CredentialOwnerId::new("tenant-1").unwrap(), id.clone()),
        CredentialStreamId::new(CredentialOwnerId::new("tenant-2").unwrap(), id)
    );
}
