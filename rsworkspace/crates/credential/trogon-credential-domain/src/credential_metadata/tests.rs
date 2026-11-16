use super::*;
use crate::{CredentialIdentity, CredentialKind, CredentialOwnerId, CredentialScope, CredentialVersion, SourceKind};

#[test]
fn exposes_the_parts_it_was_built_from() {
    let reference = CredentialRef::new(
        CredentialIdentity::derive(
            CredentialScope::source(CredentialOwnerId::static_config(), SourceKind::Discord),
            CredentialKind::BotToken,
        ),
        CredentialVersion::initial(),
    );
    let fingerprint = CredentialFingerprint::new("static-config:abc").unwrap();

    let metadata = CredentialMetadata::new(reference.clone(), CredentialStatus::Previous, fingerprint.clone());

    assert_eq!(metadata.reference(), &reference);
    assert_eq!(metadata.status(), CredentialStatus::Previous);
    assert_eq!(metadata.fingerprint(), &fingerprint);
}
