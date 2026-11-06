use super::*;
use crate::credential::commands::domain::{
    CredentialKind, CredentialOwnerId, CredentialScope, CredentialVersion, SourceKind, StorageBackend,
};

#[test]
fn exposes_the_parts_it_was_built_from() {
    let scope = CredentialScope::source(CredentialOwnerId::static_config(), SourceKind::Discord);
    let reference = CredentialRef::new(
        crate::credential::commands::domain::CredentialId::derive(&scope, CredentialKind::BotToken),
        CredentialVersion::initial(),
        &scope,
        CredentialKind::BotToken,
    );
    let fingerprint = CredentialFingerprint::new("static-config:abc").unwrap();
    let locator = StorageLocator::new("static-config:discord:bot_token").unwrap();

    let metadata = CredentialMetadata::new(
        reference.clone(),
        CredentialStatus::Previous,
        locator.clone(),
        fingerprint.clone(),
    );

    assert_eq!(metadata.reference(), &reference);
    assert_eq!(metadata.status(), CredentialStatus::Previous);
    assert_eq!(metadata.storage_locator(), &locator);
    assert_eq!(metadata.storage_locator().backend(), StorageBackend::StaticConfig);
    assert_eq!(metadata.fingerprint(), &fingerprint);
}
