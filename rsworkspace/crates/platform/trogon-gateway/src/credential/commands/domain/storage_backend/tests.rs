use super::*;

fn credential_id(raw: &str) -> CredentialId {
    CredentialId::new(raw).unwrap()
}

#[test]
fn openbao_ids_map_to_openbao() {
    let id = credential_id("openbao:tenant-1:github/primary:webhook_secret");

    assert_eq!(StorageBackend::from_credential_id(&id), Ok(StorageBackend::OpenBao));
}

#[test]
fn memory_ids_map_to_in_memory() {
    let id = credential_id("memory:github/primary:webhook_secret:1");

    assert_eq!(StorageBackend::from_credential_id(&id), Ok(StorageBackend::InMemory));
}

#[test]
fn static_config_ids_map_to_static_config() {
    let id = credential_id("static-config:github/primary:webhook_secret");

    assert_eq!(
        StorageBackend::from_credential_id(&id),
        Ok(StorageBackend::StaticConfig)
    );
}

#[test]
fn only_the_text_before_the_first_colon_decides_the_backend() {
    let id = credential_id("openbao:memory:static-config:webhook_secret");

    assert_eq!(StorageBackend::from_credential_id(&id), Ok(StorageBackend::OpenBao));
}

#[test]
fn an_id_without_a_colon_has_no_prefix() {
    let id = credential_id("webhook_secret");

    assert_eq!(
        StorageBackend::from_credential_id(&id),
        Err(StorageBackendError::MissingPrefix)
    );
}

#[test]
fn an_unknown_prefix_is_rejected() {
    let id = credential_id("vault:tenant-1:github/primary:webhook_secret");

    assert_eq!(
        StorageBackend::from_credential_id(&id),
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
            StorageBackend::from_credential_id(&credential_id(raw)),
            Err(StorageBackendError::UnknownPrefix(prefix.to_string()))
        );
    }
}

#[test]
fn display_matches_the_wire_name_of_every_backend() {
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
