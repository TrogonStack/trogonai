use super::*;

fn id() -> CredentialId {
    CredentialId::new("93171e83-3fe8-5776-ab3e-f27b7fdaed81").unwrap()
}

#[test]
fn pairs_the_owner_with_the_opaque_id() {
    let owner = CredentialOwnerId::new("tenant-1").unwrap();

    let key = CredentialKey::new(owner.clone(), id());

    assert_eq!(key.owner_id(), &owner);
    assert_eq!(key.credential_id(), &id());
    assert_eq!(key.to_string(), "tenant-1/93171e83-3fe8-5776-ab3e-f27b7fdaed81");
}

#[test]
fn the_same_id_under_another_owner_is_another_stream() {
    assert_ne!(
        CredentialKey::new(CredentialOwnerId::new("tenant-1").unwrap(), id()),
        CredentialKey::new(CredentialOwnerId::new("tenant-2").unwrap(), id())
    );
}
