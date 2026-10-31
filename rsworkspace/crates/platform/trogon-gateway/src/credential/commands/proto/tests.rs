use super::*;

const ALL_SOURCES: [SourceKind; 15] = [
    SourceKind::Datadog,
    SourceKind::Discord,
    SourceKind::GitHub,
    SourceKind::Gitlab,
    SourceKind::GooglePubSub,
    SourceKind::Incidentio,
    SourceKind::Linear,
    SourceKind::MicrosoftGraph,
    SourceKind::Notion,
    SourceKind::Plaid,
    SourceKind::Sentry,
    SourceKind::Slack,
    SourceKind::Telegram,
    SourceKind::Twitter,
    SourceKind::Zoom,
];

const ALL_KINDS: [CredentialKind; 12] = [
    CredentialKind::AppToken,
    CredentialKind::BotToken,
    CredentialKind::ClientSecret,
    CredentialKind::ClientState,
    CredentialKind::ConsumerSecret,
    CredentialKind::SigningSecret,
    CredentialKind::SigningToken,
    CredentialKind::VerificationToken,
    CredentialKind::WebhookSecret,
    CredentialKind::WebhookToken,
    CredentialKind::SecretToken,
    CredentialKind::ServiceAccountCredentials,
];

const ALL_STATUSES: [CredentialStatus; 6] = [
    CredentialStatus::Pending,
    CredentialStatus::Active,
    CredentialStatus::Previous,
    CredentialStatus::Revoked,
    CredentialStatus::Expired,
    CredentialStatus::Destroyed,
];

const ALL_BACKENDS: [(StorageBackend, &str); 3] = [
    (StorageBackend::InMemory, "memory:tenant-1:webhook_secret"),
    (StorageBackend::OpenBao, "openbao:tenant-1:webhook_secret"),
    (StorageBackend::StaticConfig, "static-config:tenant-1:webhook_secret"),
];

fn owner_id() -> CredentialOwnerId {
    CredentialOwnerId::new("tenant-1").unwrap()
}

fn credential_ref(source: SourceKind, kind: CredentialKind) -> CredentialRef {
    CredentialRef::new(
        CredentialId::new("openbao:tenant-1:webhook_secret").unwrap(),
        CredentialVersion::initial(),
        &CredentialScope::source(owner_id(), source),
        kind,
    )
}

fn default_ref() -> CredentialRef {
    credential_ref(SourceKind::GitHub, CredentialKind::WebhookSecret)
}

fn metadata(status: CredentialStatus, backend: StorageBackend, id: &str) -> CredentialMetadata {
    let reference = CredentialRef::new(
        CredentialId::new(id).unwrap(),
        CredentialVersion::initial(),
        &CredentialScope::source(owner_id(), SourceKind::GitHub),
        CredentialKind::WebhookSecret,
    );
    CredentialMetadata::new(
        reference,
        status,
        backend,
        CredentialFingerprint::new("fingerprint").unwrap(),
    )
}

fn invalid_ref_proto() -> v1::CredentialRef {
    v1::CredentialRef {
        id: String::new(),
        ..credential_ref_to_proto(&default_ref())
    }
}

fn invalid_id_error(field: &'static str) -> CredentialProtoDecodeError {
    CredentialProtoDecodeError::InvalidField {
        field: field.into(),
        reason: CredentialFieldInvalidError::CredentialId(CredentialIdError::Empty),
    }
}

#[test]
fn every_source_kind_round_trips_through_the_wire_form() {
    for source in ALL_SOURCES {
        let reference = credential_ref(source, CredentialKind::WebhookSecret);

        assert_eq!(
            decode_credential_ref("credential_ref", &credential_ref_to_proto(&reference)),
            Ok(reference)
        );
    }
}

#[test]
fn every_credential_kind_round_trips_through_the_wire_form() {
    for kind in ALL_KINDS {
        let reference = credential_ref(SourceKind::GitHub, kind);

        assert_eq!(
            decode_credential_ref("credential_ref", &credential_ref_to_proto(&reference)),
            Ok(reference)
        );
    }
}

#[test]
fn every_credential_status_round_trips_through_event_metadata() {
    for status in ALL_STATUSES {
        let expected = metadata(status, StorageBackend::OpenBao, "openbao:tenant-1:webhook_secret");

        assert_eq!(
            decode_credential_metadata("metadata", &credential_metadata_to_proto(&expected)),
            Ok(expected)
        );
    }
}

#[test]
fn every_storage_backend_round_trips_through_state_metadata() {
    for (backend, id) in ALL_BACKENDS {
        let expected = metadata(CredentialStatus::Active, backend, id);

        assert_eq!(
            decode_credential_metadata_state("metadata", &credential_metadata_to_proto_state(&expected)),
            Ok(expected)
        );
    }
}

#[test]
fn unspecified_enums_are_rejected_with_the_field_that_carried_them() {
    let mut source = credential_ref_to_proto(&default_ref());
    source.source = Some(v1::CredentialSource::CREDENTIAL_SOURCE_UNSPECIFIED.into());
    let mut kind = credential_ref_to_proto(&default_ref());
    kind.kind = Some(v1::CredentialKind::CREDENTIAL_KIND_UNSPECIFIED.into());
    let mut status = credential_metadata_to_proto(&metadata(
        CredentialStatus::Active,
        StorageBackend::OpenBao,
        "openbao:tenant-1:webhook_secret",
    ));
    status.status = Some(v1::CredentialStatus::CREDENTIAL_STATUS_UNSPECIFIED.into());
    let mut backend = credential_metadata_to_proto_state(&metadata(
        CredentialStatus::Active,
        StorageBackend::OpenBao,
        "openbao:tenant-1:webhook_secret",
    ));
    backend.storage_backend = Some(state_v1::StorageBackend::STORAGE_BACKEND_UNSPECIFIED.into());

    let unspecified = |field: &'static str, kind| CredentialProtoDecodeError::InvalidField {
        field: field.into(),
        reason: CredentialFieldInvalidError::UnspecifiedEnum(kind),
    };

    assert_eq!(
        decode_credential_ref("credential_ref", &source),
        Err(unspecified("credential_ref.source", UnspecifiedEnumKind::SourceKind))
    );
    assert_eq!(
        decode_credential_ref("credential_ref", &kind),
        Err(unspecified("credential_ref.kind", UnspecifiedEnumKind::CredentialKind))
    );
    assert_eq!(
        decode_credential_metadata("metadata", &status),
        Err(unspecified("metadata.status", UnspecifiedEnumKind::CredentialStatus))
    );
    assert_eq!(
        decode_credential_metadata_state("metadata", &backend),
        Err(unspecified(
            "metadata.storage_backend",
            UnspecifiedEnumKind::StorageBackend
        ))
    );
}

#[test]
fn decode_errors_render_the_field_and_the_unspecified_kind() {
    let kinds = [
        (UnspecifiedEnumKind::SourceKind, "source kind"),
        (UnspecifiedEnumKind::CredentialKind, "credential kind"),
        (UnspecifiedEnumKind::CredentialStatus, "credential status"),
        (UnspecifiedEnumKind::StorageBackend, "storage backend"),
    ];
    for (kind, label) in kinds {
        assert_eq!(
            CredentialFieldInvalidError::UnspecifiedEnum(kind).to_string(),
            format!("unspecified {label}")
        );
    }

    assert_eq!(
        CredentialProtoDecodeError::MissingField {
            field: CredentialFieldPath::from("metadata").child("reference")
        }
        .to_string(),
        "credential field 'metadata.reference' is missing"
    );
}

#[test]
fn event_metadata_names_the_missing_or_invalid_reference() {
    let missing = v1::CredentialMetadata::default();
    let invalid = v1::CredentialMetadata {
        reference: MessageField::some(invalid_ref_proto()),
        status: Some(v1::CredentialStatus::CREDENTIAL_STATUS_ACTIVE.into()),
        fingerprint: "fingerprint".to_string(),
    };

    assert_eq!(
        decode_credential_metadata("metadata", &missing),
        Err(CredentialProtoDecodeError::MissingField {
            field: "metadata.reference".into()
        })
    );
    assert_eq!(
        decode_credential_metadata("metadata", &invalid),
        Err(invalid_id_error("metadata.reference.id"))
    );
}

#[test]
fn event_decoders_name_an_invalid_credential_ref() {
    let reason = "openbao failed".to_string();
    let invalid = || MessageField::some(invalid_ref_proto());

    assert_eq!(
        decode_rotation_failed(&v1::CredentialRotationFailed {
            credential_ref: invalid(),
            reason: reason.clone(),
        }),
        Err(invalid_id_error("credential_ref.id"))
    );
    assert_eq!(
        decode_rotated(&v1::CredentialRotated {
            previous_credential_ref: invalid(),
            metadata: MessageField::some(credential_metadata_to_proto(&metadata(
                CredentialStatus::Active,
                StorageBackend::OpenBao,
                "openbao:tenant-1:webhook_secret",
            ))),
        }),
        Err(invalid_id_error("previous_credential_ref.id"))
    );
    assert_eq!(
        decode_destroy_requested(&v1::CredentialDestroyRequested {
            credential_ref: invalid(),
            reason: reason.clone(),
        }),
        Err(invalid_id_error("credential_ref.id"))
    );
    assert_eq!(
        decode_destroy_failed(&v1::CredentialDestroyFailed {
            credential_ref: invalid(),
            reason,
        }),
        Err(invalid_id_error("credential_ref.id"))
    );
}

#[test]
fn state_decoders_name_an_invalid_credential_ref() {
    let invalid = || MessageField::some(invalid_ref_proto());

    assert_eq!(
        decode_destroy_requested_state(&state_v1::DestroyRequestedCredentialState {
            credential_ref: invalid(),
            reason: "cleanup".to_string(),
        }),
        Err(invalid_id_error("destroy_requested.credential_ref.id"))
    );
    assert_eq!(
        decode_cleanup_failed_state(&state_v1::CleanupFailedCredentialState {
            credential_ref: invalid(),
            reason: "openbao failed".to_string(),
        }),
        Err(invalid_id_error("cleanup_failed.credential_ref.id"))
    );
}
