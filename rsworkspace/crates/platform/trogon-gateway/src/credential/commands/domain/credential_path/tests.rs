use super::*;

fn owner() -> CredentialOwnerId {
    CredentialOwnerId::new("tenant-1").unwrap()
}

fn locator(value: &str) -> StorageLocator {
    StorageLocator::new(value).unwrap()
}

#[test]
fn renders_the_managed_subtree_layout() {
    let path = CredentialPath::new(owner(), locator("openbao:tenant-1:github:webhook_secret"));

    assert_eq!(
        path.to_string(),
        "trogonai/tenant-1/credentials/openbao:tenant-1:github:webhook_secret"
    );
}

#[test]
fn segments_are_unjoined_so_a_caller_can_encode_each_one() {
    let path = CredentialPath::new(owner(), locator("openbao:tenant-1:github/acme:webhook_secret"));

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
