use super::*;
use crate::credential::commands::domain::{
    CredentialId, CredentialKind, CredentialOwnerId, CredentialScope, CredentialVersion, SourceKind,
};
use crate::source_integration_id::SourceIntegrationId;

#[test]
fn accepts_a_storage_safe_locator_and_names_its_backend() {
    let locator = StorageLocator::new("static-config:github/primary:webhook_secret").unwrap();

    assert_eq!(locator.as_str(), "static-config:github/primary:webhook_secret");
    assert_eq!(locator.backend(), StorageBackend::StaticConfig);
    assert_eq!(locator.to_string(), "static-config:github/primary:webhook_secret");
}

#[test]
fn rejects_empty_locator() {
    assert_eq!(StorageLocator::new(""), Err(StorageLocatorError::Empty));
}

#[test]
fn rejects_spaces() {
    assert_eq!(
        StorageLocator::new("openbao:github primary"),
        Err(StorageLocatorError::InvalidCharacter(' '))
    );
}

#[test]
fn rejects_locator_over_the_maximum_length() {
    let too_long = format!("openbao:{}", "a".repeat(MAX_STORAGE_LOCATOR_LEN));

    assert_eq!(
        StorageLocator::new(&too_long),
        Err(StorageLocatorError::TooLong(too_long.len()))
    );
}

#[test]
fn rejects_dot_path_segments_so_the_locator_stays_inside_its_collection() {
    for value in ["openbao:/.", "openbao:/..", "openbao:a/./b", "openbao:a/.."] {
        assert_eq!(
            StorageLocator::new(value),
            Err(StorageLocatorError::DotSegment),
            "{value}"
        );
    }
    assert!(StorageLocator::new("openbao:github/.primary").is_ok());
}

#[test]
fn rejects_a_locator_no_backend_claims() {
    assert_eq!(
        StorageLocator::new("webhook_secret"),
        Err(StorageLocatorError::Backend(StorageBackendError::MissingPrefix))
    );
    assert_eq!(
        StorageLocator::new("vault:tenant-1:github:webhook_secret"),
        Err(StorageLocatorError::Backend(StorageBackendError::UnknownPrefix(
            "vault".to_string()
        )))
    );
}

fn reference() -> CredentialRef {
    let scope = CredentialScope::integration(
        CredentialOwnerId::new("tenant-1").unwrap(),
        SourceKind::GitHub,
        SourceIntegrationId::new("acme").unwrap(),
    );
    CredentialRef::new(
        CredentialId::derive(&scope, CredentialKind::WebhookSecret),
        CredentialVersion::initial(),
        &scope,
        CredentialKind::WebhookSecret,
    )
}

#[test]
fn the_openbao_locator_is_the_leaf_the_id_used_to_be() {
    let locator = StorageLocator::openbao(&reference()).unwrap();

    assert_eq!(locator.as_str(), "openbao:tenant-1:github/acme:webhook_secret");
    assert_eq!(locator.backend(), StorageBackend::OpenBao);
}

#[test]
fn the_in_memory_locator_names_owner_scope_and_kind() {
    let locator = StorageLocator::in_memory(&reference()).unwrap();

    assert_eq!(locator.as_str(), "memory:tenant-1:github/acme:webhook_secret");
    assert_eq!(locator.backend(), StorageBackend::InMemory);
}

#[test]
fn the_static_config_locator_names_scope_and_kind() {
    let locator = StorageLocator::static_config(&reference()).unwrap();

    assert_eq!(locator.as_str(), "static-config:github/acme:webhook_secret");
    assert_eq!(locator.backend(), StorageBackend::StaticConfig);
}
