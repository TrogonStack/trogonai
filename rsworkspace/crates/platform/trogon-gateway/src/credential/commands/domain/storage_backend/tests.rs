use super::*;

#[test]
fn openbao_locators_map_to_openbao() {
    assert_eq!(
        StorageBackend::from_locator("openbao:tenant-1:github/primary:webhook_secret"),
        Ok(StorageBackend::OpenBao)
    );
}

#[test]
fn memory_locators_map_to_in_memory() {
    assert_eq!(
        StorageBackend::from_locator("memory:tenant-1:github/primary:webhook_secret"),
        Ok(StorageBackend::InMemory)
    );
}

#[test]
fn static_config_locators_map_to_static_config() {
    assert_eq!(
        StorageBackend::from_locator("static-config:github/primary:webhook_secret"),
        Ok(StorageBackend::StaticConfig)
    );
}

#[test]
fn only_the_text_before_the_first_colon_decides_the_backend() {
    assert_eq!(
        StorageBackend::from_locator("openbao:memory:static-config:webhook_secret"),
        Ok(StorageBackend::OpenBao)
    );
}

#[test]
fn a_locator_without_a_colon_has_no_prefix() {
    assert_eq!(
        StorageBackend::from_locator("webhook_secret"),
        Err(StorageBackendError::MissingPrefix)
    );
}

#[test]
fn an_unknown_prefix_is_rejected() {
    assert_eq!(
        StorageBackend::from_locator("vault:tenant-1:github/primary:webhook_secret"),
        Err(StorageBackendError::UnknownPrefix("vault".to_string()))
    );
}

#[test]
fn a_prefix_must_match_exactly() {
    for (raw, prefix) in [
        ("OpenBao:tenant-1:github:webhook_secret", "OpenBao"),
        ("openbao-eu:tenant-1:github:webhook_secret", "openbao-eu"),
        ("in_memory:github:webhook_secret:1", "in_memory"),
        ("static_config:github:webhook_secret", "static_config"),
        (":github:webhook_secret", ""),
    ] {
        assert_eq!(
            StorageBackend::from_locator(raw),
            Err(StorageBackendError::UnknownPrefix(prefix.to_string()))
        );
    }
}

#[test]
fn display_names_every_backend() {
    let expected = [
        (StorageBackend::InMemory, "in_memory"),
        (StorageBackend::OpenBao, "openbao"),
        (StorageBackend::StaticConfig, "static_config"),
    ];

    for (backend, name) in expected {
        assert_eq!(backend.as_str(), name);
        assert_eq!(backend.to_string(), name);
    }
}
