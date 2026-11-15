use trogon_credential_domain::{
    CredentialDestroyReasonError, CredentialFailureReasonError, CredentialFingerprintError, CredentialIdError,
    CredentialOwnerIdError, CredentialVersionError, SourceIntegrationIdError,
};

use super::*;

const RECORDED_ID: &str = "6f9619ff-8b86-4011-b42d-00c04fc964ff";

fn owner_id() -> CredentialOwnerId {
    CredentialOwnerId::new("tenant-1").unwrap()
}

fn identity() -> CredentialIdentity {
    CredentialIdentity::derive(
        CredentialScope::source(owner_id(), SourceKind::GitHub),
        CredentialKind::WebhookSecret,
    )
}

fn version(value: u64) -> CredentialVersion {
    CredentialVersion::new(value).unwrap()
}

fn credential_ref(value: u64) -> CredentialRef {
    CredentialRef::new(identity(), version(value))
}

fn fingerprint() -> CredentialFingerprint {
    CredentialFingerprint::new("fingerprint").unwrap()
}

fn failure() -> CredentialFailureReason {
    CredentialFailureReason::new("openbao failed").unwrap()
}

fn destroy_reason() -> CredentialDestroyReason {
    CredentialDestroyReason::new("cleanup").unwrap()
}

fn metadata(value: u64) -> CredentialMetadata {
    CredentialMetadata::new(credential_ref(value), CredentialStatus::Active, fingerprint())
}

fn wire_identity() -> v1::CredentialIdentity {
    identity_to_proto(&identity())
}

fn wire_key() -> v1::CredentialKey {
    key_to_proto(&identity().key())
}

/// A key whose credential id is not a canonical UUID.
fn invalid_wire_key() -> v1::CredentialKey {
    v1::CredentialKey {
        credential_id: String::new(),
        ..wire_key()
    }
}

fn missing(field: &'static str) -> CredentialProtoDecodeError {
    CredentialProtoDecodeError::MissingField { field: field.into() }
}

fn invalid(field: &'static str, reason: impl Into<CredentialFieldInvalidError>) -> CredentialProtoDecodeError {
    CredentialProtoDecodeError::InvalidField {
        field: field.into(),
        reason: reason.into(),
    }
}

#[test]
fn every_source_and_kind_round_trips_through_the_recorded_identity() {
    for source in SourceKind::ALL {
        for kind in CredentialKind::ALL {
            let identity = CredentialIdentity::derive(CredentialScope::source(owner_id(), source), kind);

            assert_eq!(decode_identity("identity", &identity_to_proto(&identity)), Ok(identity));
        }
    }
}

#[test]
fn an_integration_scope_round_trips_through_the_recorded_identity() {
    let identity = CredentialIdentity::derive(
        CredentialScope::integration(
            owner_id(),
            SourceKind::GitHub,
            SourceIntegrationId::new("primary").unwrap(),
        ),
        CredentialKind::WebhookSecret,
    );
    let wire = identity_to_proto(&identity);

    assert_eq!(wire.scope_key, "github/primary");
    assert_eq!(decode_identity("identity", &wire), Ok(identity));
}

#[test]
fn the_recorded_id_is_read_back_as_recorded_and_never_derived_again() {
    let recorded = CredentialIdentity::new(
        CredentialId::new(RECORDED_ID).unwrap(),
        CredentialScope::source(owner_id(), SourceKind::GitHub),
        CredentialKind::WebhookSecret,
        CredentialIdentityVersion::V1,
    );
    let wire = identity_to_proto(&recorded);

    assert_eq!(wire.credential_id, RECORDED_ID);
    assert_eq!(wire.owner_id, "tenant-1");
    assert_eq!(
        wire.identity_version,
        Some(EnumValue::from(
            v1::CredentialIdentityVersion::CREDENTIAL_IDENTITY_VERSION_V1
        ))
    );
    assert_eq!(decode_identity("identity", &wire), Ok(recorded));
}

#[test]
fn a_key_round_trips_through_the_wire_form() {
    let key = identity().key();

    assert_eq!(decode_key("key", &key_to_proto(&key)), Ok(key));
}

#[test]
fn an_identity_names_the_field_that_does_not_decode() {
    let cases = [
        (
            v1::CredentialIdentity {
                credential_id: "not-a-uuid".to_string(),
                ..wire_identity()
            },
            invalid("identity.credential_id", CredentialIdError::InvalidLength(10)),
        ),
        (
            v1::CredentialIdentity {
                owner_id: String::new(),
                ..wire_identity()
            },
            invalid("identity.owner_id", CredentialOwnerIdError::Empty),
        ),
        (
            v1::CredentialIdentity {
                source: None,
                ..wire_identity()
            },
            missing("identity.source"),
        ),
        (
            v1::CredentialIdentity {
                kind: None,
                ..wire_identity()
            },
            missing("identity.kind"),
        ),
        (
            v1::CredentialIdentity {
                identity_version: None,
                ..wire_identity()
            },
            missing("identity.identity_version"),
        ),
        (
            v1::CredentialIdentity {
                scope_key: String::new(),
                ..wire_identity()
            },
            invalid("identity.scope_key", CredentialFieldInvalidError::EmptyScopeKey),
        ),
        (
            v1::CredentialIdentity {
                scope_key: "gitlab".to_string(),
                ..wire_identity()
            },
            invalid(
                "identity.scope_key",
                CredentialFieldInvalidError::ScopeKeyMismatch {
                    scope_key: "gitlab".to_string(),
                    source_kind: "github",
                },
            ),
        ),
        (
            v1::CredentialIdentity {
                scope_key: "github/bad!".to_string(),
                ..wire_identity()
            },
            invalid("identity.scope_key", SourceIntegrationIdError::InvalidCharacter('!')),
        ),
    ];

    for (wire, expected) in cases {
        assert_eq!(decode_identity("identity", &wire), Err(expected));
    }
}

#[test]
fn unspecified_enums_are_rejected_with_the_field_that_carried_them() {
    let cases = [
        (
            v1::CredentialIdentity {
                source: Some(v1::CredentialSource::CREDENTIAL_SOURCE_UNSPECIFIED.into()),
                ..wire_identity()
            },
            "identity.source",
            UnspecifiedEnumKind::SourceKind,
        ),
        (
            v1::CredentialIdentity {
                kind: Some(v1::CredentialKind::CREDENTIAL_KIND_UNSPECIFIED.into()),
                ..wire_identity()
            },
            "identity.kind",
            UnspecifiedEnumKind::CredentialKind,
        ),
        (
            v1::CredentialIdentity {
                identity_version: Some(v1::CredentialIdentityVersion::CREDENTIAL_IDENTITY_VERSION_UNSPECIFIED.into()),
                ..wire_identity()
            },
            "identity.identity_version",
            UnspecifiedEnumKind::CredentialIdentityVersion,
        ),
    ];

    for (wire, field, kind) in cases {
        assert_eq!(
            decode_identity("identity", &wire),
            Err(invalid(field, CredentialFieldInvalidError::UnspecifiedEnum(kind)))
        );
    }
}

#[test]
fn unknown_enum_values_are_rejected_with_the_value_read() {
    let cases = [
        (
            v1::CredentialIdentity {
                source: Some(EnumValue::Unknown(99)),
                ..wire_identity()
            },
            "identity.source",
        ),
        (
            v1::CredentialIdentity {
                kind: Some(EnumValue::Unknown(99)),
                ..wire_identity()
            },
            "identity.kind",
        ),
        (
            v1::CredentialIdentity {
                identity_version: Some(EnumValue::Unknown(99)),
                ..wire_identity()
            },
            "identity.identity_version",
        ),
    ];

    for (wire, field) in cases {
        assert_eq!(
            decode_identity("identity", &wire),
            Err(invalid(field, CredentialFieldInvalidError::UnknownEnumValue(99)))
        );
    }
}

#[test]
fn a_key_names_the_field_that_does_not_decode() {
    assert_eq!(
        decode_key("key", &invalid_wire_key()),
        Err(invalid("key.credential_id", CredentialIdError::Empty))
    );
    assert_eq!(
        decode_key(
            "key",
            &v1::CredentialKey {
                owner_id: "a/b".to_string(),
                ..wire_key()
            }
        ),
        Err(invalid("key.owner_id", CredentialOwnerIdError::InvalidCharacter('/')))
    );
}

#[test]
fn a_version_must_be_present_and_not_zero() {
    assert_eq!(decode_version("version", Some(3)), Ok(version(3)));
    assert_eq!(decode_version("version", None), Err(missing("version")));
    assert_eq!(
        decode_version("version", Some(0)),
        Err(invalid("version", CredentialVersionError::Zero))
    );
}

#[test]
fn every_event_round_trips_through_its_decoder() {
    let key = identity().key();

    assert_eq!(
        decode_write_requested(&write_requested_to_proto(&identity())),
        Ok(identity())
    );
    assert_eq!(
        decode_write_failed(&write_failed_to_proto(&key, &failure())),
        Ok((key.clone(), failure()))
    );
    assert_eq!(
        decode_activated(&activated_to_proto(&metadata(1))),
        Ok((key.clone(), version(1), fingerprint()))
    );
    assert_eq!(
        decode_rotation_requested(&rotation_requested_to_proto(&credential_ref(1))),
        Ok((key.clone(), version(1)))
    );
    assert_eq!(
        decode_rotation_failed(&rotation_failed_to_proto(&credential_ref(1), &failure())),
        Ok((key.clone(), version(1), failure()))
    );
    assert_eq!(
        decode_rotated(&rotated_to_proto(&credential_ref(1), &metadata(2))),
        Ok((key.clone(), version(1), version(2), fingerprint()))
    );
    assert_eq!(
        decode_revoked(&revoked_to_proto(&credential_ref(1))),
        Ok((key.clone(), version(1)))
    );
    assert_eq!(
        decode_destroy_requested(&destroy_requested_to_proto(&credential_ref(1), &destroy_reason())),
        Ok((key.clone(), version(1), destroy_reason()))
    );
    assert_eq!(
        decode_destroyed(&destroyed_to_proto(&credential_ref(1))),
        Ok((key.clone(), version(1)))
    );
    assert_eq!(
        decode_destroy_failed(&destroy_failed_to_proto(&credential_ref(1), &failure())),
        Ok((key, version(1), failure()))
    );
}

#[test]
fn only_the_write_request_carries_the_identity() {
    let wire = write_requested_to_proto(&identity());

    assert_eq!(wire.identity.as_option(), Some(&wire_identity()));
    assert_eq!(activated_to_proto(&metadata(1)).key.as_option(), Some(&wire_key()));
}

#[test]
fn every_event_names_the_credential_it_belongs_to() {
    let key = identity().key();
    let events: [CredentialEventCase; 10] = [
        write_requested_to_proto(&identity()).into(),
        write_failed_to_proto(&key, &failure()).into(),
        activated_to_proto(&metadata(1)).into(),
        rotation_requested_to_proto(&credential_ref(1)).into(),
        rotation_failed_to_proto(&credential_ref(1), &failure()).into(),
        rotated_to_proto(&credential_ref(1), &metadata(2)).into(),
        revoked_to_proto(&credential_ref(1)).into(),
        destroy_requested_to_proto(&credential_ref(1), &destroy_reason()).into(),
        destroyed_to_proto(&credential_ref(1)).into(),
        destroy_failed_to_proto(&credential_ref(1), &failure()).into(),
    ];

    for event in events {
        assert_eq!(event_key(&event), Ok(key.clone()));
    }
}

#[test]
fn an_event_whose_identity_or_key_is_missing_names_the_field() {
    let events: [(CredentialEventCase, &str); 10] = [
        (v1::CredentialWriteRequested::default().into(), "identity"),
        (v1::CredentialWriteFailed::default().into(), "key"),
        (v1::CredentialActivated::default().into(), "key"),
        (v1::CredentialRotationRequested::default().into(), "key"),
        (v1::CredentialRotationFailed::default().into(), "key"),
        (v1::CredentialRotated::default().into(), "key"),
        (v1::CredentialRevoked::default().into(), "key"),
        (v1::CredentialDestroyRequested::default().into(), "key"),
        (v1::CredentialDestroyed::default().into(), "key"),
        (v1::CredentialDestroyFailed::default().into(), "key"),
    ];

    for (event, field) in events {
        assert_eq!(event_key(&event), Err(missing(field)));
    }
}

#[test]
fn an_event_with_an_invalid_key_names_the_key_field() {
    let key = || MessageField::some(invalid_wire_key());
    let expected = || invalid("key.credential_id", CredentialIdError::Empty);

    assert_eq!(
        decode_write_failed(&v1::CredentialWriteFailed {
            key: key(),
            ..write_failed_to_proto(&identity().key(), &failure())
        }),
        Err(expected())
    );
    assert_eq!(
        decode_activated(&v1::CredentialActivated {
            key: key(),
            ..activated_to_proto(&metadata(1))
        }),
        Err(expected())
    );
    assert_eq!(
        decode_rotated(&v1::CredentialRotated {
            key: key(),
            ..rotated_to_proto(&credential_ref(1), &metadata(2))
        }),
        Err(expected())
    );
    assert_eq!(
        decode_destroy_failed(&v1::CredentialDestroyFailed {
            key: key(),
            ..destroy_failed_to_proto(&credential_ref(1), &failure())
        }),
        Err(expected())
    );
}

#[test]
fn a_write_request_with_an_invalid_identity_names_the_identity_field() {
    assert_eq!(
        decode_write_requested(&v1::CredentialWriteRequested {
            identity: MessageField::some(v1::CredentialIdentity {
                credential_id: String::new(),
                ..wire_identity()
            }),
        }),
        Err(invalid("identity.credential_id", CredentialIdError::Empty))
    );
}

#[test]
fn an_event_names_the_version_reason_or_fingerprint_that_does_not_decode() {
    assert_eq!(
        decode_activated(&v1::CredentialActivated {
            version: None,
            ..activated_to_proto(&metadata(1))
        }),
        Err(missing("version"))
    );
    assert_eq!(
        decode_activated(&v1::CredentialActivated {
            fingerprint: String::new(),
            ..activated_to_proto(&metadata(1))
        }),
        Err(invalid("fingerprint", CredentialFingerprintError::Empty))
    );
    assert_eq!(
        decode_rotated(&v1::CredentialRotated {
            previous_version: Some(0),
            ..rotated_to_proto(&credential_ref(1), &metadata(2))
        }),
        Err(invalid("previous_version", CredentialVersionError::Zero))
    );
    assert_eq!(
        decode_write_failed(&v1::CredentialWriteFailed {
            reason: String::new(),
            ..write_failed_to_proto(&identity().key(), &failure())
        }),
        Err(invalid("reason", CredentialFailureReasonError::Empty))
    );
    assert_eq!(
        decode_destroy_requested(&v1::CredentialDestroyRequested {
            reason: String::new(),
            ..destroy_requested_to_proto(&credential_ref(1), &destroy_reason())
        }),
        Err(invalid("reason", CredentialDestroyReasonError::Empty))
    );
}

#[test]
fn every_state_round_trips_through_its_decoder() {
    assert_eq!(
        decode_pending_write_state(&pending_write_to_proto_state(&identity())),
        Ok(identity())
    );
    assert_eq!(
        decode_write_failed_state(&write_failed_to_proto_state(&identity(), &failure())),
        Ok((identity(), failure()))
    );
    assert_eq!(
        active_credential_ref(&active_state_to_proto(&credential_ref(2), &fingerprint(), &[])),
        Ok(credential_ref(2))
    );
    assert_eq!(
        decode_revoked_state(&revoked_to_proto_state(&credential_ref(2))),
        Ok(credential_ref(2))
    );
    assert_eq!(
        decode_destroy_requested_state(&destroy_requested_to_proto_state(&credential_ref(2), &destroy_reason())),
        Ok((credential_ref(2), destroy_reason()))
    );
    assert_eq!(
        decode_destroyed_state(&destroyed_to_proto_state(&credential_ref(2))),
        Ok(credential_ref(2))
    );
    assert_eq!(
        decode_cleanup_failed_state(&cleanup_failed_to_proto_state(&credential_ref(2), &failure())),
        Ok((credential_ref(2), failure()))
    );
}

#[test]
fn an_active_state_shares_its_identity_with_every_version_it_replaced() {
    let active = active_state_to_proto(&credential_ref(3), &fingerprint(), &[version(1), version(2)]);

    assert_eq!(active.previous_versions, vec![1, 2]);
    assert_eq!(
        decode_active_state("active", &active),
        Ok((metadata(3), vec![credential_ref(1), credential_ref(2)]))
    );
}

#[test]
fn an_active_state_names_the_field_that_does_not_decode() {
    let active = || active_state_to_proto(&credential_ref(2), &fingerprint(), &[version(1)]);

    assert_eq!(
        decode_active_state(
            "active",
            &state_v1::ActiveCredentialState {
                identity: MessageField::none(),
                ..active()
            }
        ),
        Err(missing("active.identity"))
    );
    assert_eq!(
        decode_active_state(
            "active",
            &state_v1::ActiveCredentialState {
                version: None,
                ..active()
            }
        ),
        Err(missing("active.version"))
    );
    assert_eq!(
        decode_active_state(
            "active",
            &state_v1::ActiveCredentialState {
                fingerprint: String::new(),
                ..active()
            }
        ),
        Err(invalid("active.fingerprint", CredentialFingerprintError::Empty))
    );
    assert_eq!(
        decode_active_state(
            "active",
            &state_v1::ActiveCredentialState {
                previous_versions: vec![0],
                ..active()
            }
        ),
        Err(invalid("active.previous_versions", CredentialVersionError::Zero))
    );
}

#[test]
fn a_state_without_its_identity_names_the_missing_field() {
    assert_eq!(
        decode_pending_write_state(&state_v1::PendingCredentialWriteState::default()),
        Err(missing("pending_write.identity"))
    );
    assert_eq!(
        decode_write_failed_state(&state_v1::FailedCredentialWriteState::default()),
        Err(missing("write_failed.identity"))
    );
    assert_eq!(
        active_credential_ref(&state_v1::ActiveCredentialState::default()),
        Err(missing("active.identity"))
    );
    assert_eq!(
        decode_revoked_state(&state_v1::RevokedCredentialState::default()),
        Err(missing("revoked.identity"))
    );
    assert_eq!(
        decode_destroy_requested_state(&state_v1::DestroyRequestedCredentialState::default()),
        Err(missing("destroy_requested.identity"))
    );
    assert_eq!(
        decode_destroyed_state(&state_v1::DestroyedCredentialState::default()),
        Err(missing("destroyed.identity"))
    );
    assert_eq!(
        decode_cleanup_failed_state(&state_v1::CleanupFailedCredentialState::default()),
        Err(missing("cleanup_failed.identity"))
    );
}

#[test]
fn a_state_names_the_version_or_reason_that_does_not_decode() {
    assert_eq!(
        decode_revoked_state(&state_v1::RevokedCredentialState {
            version: None,
            ..revoked_to_proto_state(&credential_ref(1))
        }),
        Err(missing("revoked.version"))
    );
    assert_eq!(
        decode_write_failed_state(&state_v1::FailedCredentialWriteState {
            reason: String::new(),
            ..write_failed_to_proto_state(&identity(), &failure())
        }),
        Err(invalid("write_failed.reason", CredentialFailureReasonError::Empty))
    );
    assert_eq!(
        decode_destroy_requested_state(&state_v1::DestroyRequestedCredentialState {
            reason: String::new(),
            ..destroy_requested_to_proto_state(&credential_ref(1), &destroy_reason())
        }),
        Err(invalid("destroy_requested.reason", CredentialDestroyReasonError::Empty))
    );
    assert_eq!(
        decode_cleanup_failed_state(&state_v1::CleanupFailedCredentialState {
            reason: String::new(),
            ..cleanup_failed_to_proto_state(&credential_ref(1), &failure())
        }),
        Err(invalid("cleanup_failed.reason", CredentialFailureReasonError::Empty))
    );
}

#[test]
fn a_rotation_pending_state_wraps_the_active_state() {
    let active = active_state_to_proto(&credential_ref(1), &fingerprint(), &[]);

    assert_eq!(
        rotation_pending_to_proto_state(active.clone()).active.as_option(),
        Some(&active)
    );
}

#[test]
fn decode_errors_render_the_field_and_the_reason() {
    let kinds = [
        (UnspecifiedEnumKind::SourceKind, "source kind"),
        (UnspecifiedEnumKind::CredentialKind, "credential kind"),
        (
            UnspecifiedEnumKind::CredentialIdentityVersion,
            "credential identity version",
        ),
    ];
    for (kind, label) in kinds {
        assert_eq!(
            CredentialFieldInvalidError::UnspecifiedEnum(kind).to_string(),
            format!("unspecified {label}")
        );
    }

    let cases = [
        (
            missing("identity.source").to_string(),
            "credential field 'identity.source' is missing",
        ),
        (
            invalid("identity.scope_key", CredentialFieldInvalidError::EmptyScopeKey).to_string(),
            "credential field 'identity.scope_key' is invalid: credential scope key must not be empty",
        ),
        (
            CredentialFieldInvalidError::ScopeKeyMismatch {
                scope_key: "gitlab".to_string(),
                source_kind: "github",
            }
            .to_string(),
            "scope key 'gitlab' does not match source 'github'",
        ),
        (
            CredentialFieldInvalidError::UnknownEnumValue(99).to_string(),
            "unknown enum value 99",
        ),
    ];
    for (actual, expected) in cases {
        assert_eq!(actual, expected);
    }
}

#[test]
fn a_field_path_names_each_nested_field() {
    let identity = CredentialFieldPath::from("pending_write").child("identity");

    assert_eq!(identity, "pending_write.identity");
    assert_eq!(
        identity.child("credential_id").to_string(),
        "pending_write.identity.credential_id"
    );
    assert_eq!(CredentialFieldPath::from(&identity), identity);
}
