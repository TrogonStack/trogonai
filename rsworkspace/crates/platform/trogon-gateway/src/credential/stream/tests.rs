use super::*;

fn credential_id(raw: &str) -> CredentialId {
    CredentialId::new(raw).unwrap()
}

#[test]
fn key_derivation_is_deterministic_for_the_same_credential_id() {
    let first = CredentialKey::derive(&credential_id("openbao:tenant-1:github/primary:webhook_secret"));
    let second = CredentialKey::derive(&credential_id("openbao:tenant-1:github/primary:webhook_secret"));

    assert_eq!(first, second);
}

#[test]
fn key_derivation_keeps_nats_unsafe_ids_out_of_subject_tokens() {
    let key = CredentialKey::derive(&credential_id("openbao:tenant-1:github/primary:webhook_secret"));
    let subject = CredentialSubject::event(&key).unwrap();

    assert_eq!(
        subject.as_str(),
        format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.{}", key.simple())
    );
    assert_eq!(subject.key(), &key);
    assert_eq!(key.simple().len(), 32);
    assert!(key.simple().chars().all(|ch| ch.is_ascii_hexdigit()));
}

#[test]
fn stream_id_routing_preserves_key_derivation() {
    let raw = "openbao:tenant-1:github/primary:webhook_secret";
    let stream_id = CredentialStreamId::from(raw);

    assert_eq!(
        CredentialKey::for_stream(&stream_id),
        CredentialKey::derive(&credential_id(raw))
    );
}

#[test]
fn stream_config_matches_credential_contract() {
    let config = stream_config();

    assert_eq!(config.name, CREDENTIAL_STREAM);
    assert_eq!(config.subjects, vec![format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.>")]);
    assert!(config.allow_atomic_publish);
}
