use super::*;
use crate::credential::commands::domain::{CredentialKind, CredentialScope, CredentialVersion, SourceKind};
use crate::source_integration_id::SourceIntegrationId;

fn owner() -> CredentialOwnerId {
    CredentialOwnerId::new("tenant-1").unwrap()
}

#[test]
fn renders_the_managed_subtree_layout() {
    let path = CredentialPath::new(
        owner(),
        CredentialId::new("openbao:tenant-1:github:webhook_secret").unwrap(),
    );

    assert_eq!(
        path.to_string(),
        "trogonai/tenant-1/credentials/openbao:tenant-1:github:webhook_secret"
    );
}

#[test]
fn segments_are_unjoined_so_a_caller_can_encode_each_one() {
    let path = CredentialPath::new(
        owner(),
        CredentialId::new("openbao:tenant-1:github/acme:webhook_secret").unwrap(),
    );

    assert_eq!(
        path.segments(),
        [
            "trogonai",
            "tenant-1",
            "credentials",
            "openbao:tenant-1:github/acme:webhook_secret",
        ]
    );
}

#[test]
fn a_credential_ref_locates_the_same_path_for_every_version() {
    let scope = CredentialScope::integration(owner(), SourceKind::GitHub, SourceIntegrationId::new("acme").unwrap());
    let id = CredentialId::new("openbao:tenant-1:github/acme:webhook_secret").unwrap();
    let first = CredentialRef::new(
        id.clone(),
        CredentialVersion::initial(),
        &scope,
        CredentialKind::WebhookSecret,
    );
    let rotated = first.next_version();

    assert_eq!(CredentialPath::from(&first), CredentialPath::from(&rotated));
}
