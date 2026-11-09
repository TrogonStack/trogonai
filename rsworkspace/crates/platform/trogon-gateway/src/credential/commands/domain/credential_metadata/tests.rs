use super::*;
use crate::credential::commands::domain::{
    CredentialId, CredentialKind, CredentialOwnerId, CredentialScope, CredentialVersion, SourceKind,
};

#[test]
fn exposes_the_parts_it_was_built_from() {
    let scope = CredentialScope::source(CredentialOwnerId::static_config(), SourceKind::Discord);
    let reference = CredentialRef::new(
        CredentialId::new("static-config:discord:bot_token").unwrap(),
        CredentialVersion::initial(),
        &scope,
        CredentialKind::BotToken,
    );
    let fingerprint = CredentialFingerprint::new("static-config:abc").unwrap();

    let metadata = CredentialMetadata::new(
        reference.clone(),
        CredentialStatus::Previous,
        StorageBackend::StaticConfig,
        fingerprint.clone(),
    );

    assert_eq!(metadata.reference(), &reference);
    assert_eq!(metadata.status(), CredentialStatus::Previous);
    assert_eq!(metadata.storage_backend(), StorageBackend::StaticConfig);
    assert_eq!(metadata.fingerprint(), &fingerprint);
}
