use super::*;

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
