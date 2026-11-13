use super::*;
use crate::{CredentialId, CredentialOwnerId};

fn key() -> CredentialKey {
    CredentialKey::new(
        CredentialOwnerId::new("tenant-1").unwrap(),
        CredentialId::new("93171e83-3fe8-5776-ab3e-f27b7fdaed81").unwrap(),
    )
}

#[test]
fn renders_the_managed_subtree_layout() {
    assert_eq!(
        CredentialPath::new(key()).to_string(),
        "trogonai/tenant-1/credentials/93171e83-3fe8-5776-ab3e-f27b7fdaed81"
    );
}

#[test]
fn segments_are_unjoined_so_a_caller_can_encode_each_one() {
    assert_eq!(
        CredentialPath::new(key()).segments(),
        [
            "trogonai",
            "tenant-1",
            "credentials",
            "93171e83-3fe8-5776-ab3e-f27b7fdaed81",
        ]
    );
}
