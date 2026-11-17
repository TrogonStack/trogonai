use std::fmt::Debug;

use buffa::{EnumValue, Message, MessageName};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::gateway_tests::{
    CREDENTIAL_ID, active_state, command_response, events, identity, identity_with, key, snapshots,
};
use crate::gateway::credentials::{checkpoints_v1, idempotency_v1, state_v1, v1};

fn assert_debug_names_type<M: Message + Default + Debug>(name: &str) {
    let rendered = format!("{:?}", M::default());
    assert!(rendered.starts_with(name), "{rendered}");
}

#[test]
fn debug_output_names_every_message() {
    assert_debug_names_type::<v1::CredentialIdentity>("CredentialIdentity");
    assert_debug_names_type::<v1::CredentialKey>("CredentialKey");
    assert_debug_names_type::<v1::CredentialEvent>("CredentialEvent");
    assert_debug_names_type::<v1::CredentialWriteRequested>("CredentialWriteRequested");
    assert_debug_names_type::<v1::CredentialWriteFailed>("CredentialWriteFailed");
    assert_debug_names_type::<v1::CredentialActivated>("CredentialActivated");
    assert_debug_names_type::<v1::CredentialRotationRequested>("CredentialRotationRequested");
    assert_debug_names_type::<v1::CredentialRotationFailed>("CredentialRotationFailed");
    assert_debug_names_type::<v1::CredentialRotated>("CredentialRotated");
    assert_debug_names_type::<v1::CredentialRevoked>("CredentialRevoked");
    assert_debug_names_type::<v1::CredentialDestroyRequested>("CredentialDestroyRequested");
    assert_debug_names_type::<v1::CredentialDestroyed>("CredentialDestroyed");
    assert_debug_names_type::<v1::CredentialDestroyFailed>("CredentialDestroyFailed");
    assert_debug_names_type::<state_v1::CredentialStateSnapshot>("CredentialStateSnapshot");
    assert_debug_names_type::<state_v1::CredentialMissingState>("CredentialMissingState");
    assert_debug_names_type::<state_v1::PendingCredentialWriteState>("PendingCredentialWriteState");
    assert_debug_names_type::<state_v1::ActiveCredentialState>("ActiveCredentialState");
    assert_debug_names_type::<state_v1::FailedCredentialWriteState>("FailedCredentialWriteState");
    assert_debug_names_type::<state_v1::RotationPendingCredentialState>("RotationPendingCredentialState");
    assert_debug_names_type::<state_v1::RevokedCredentialState>("RevokedCredentialState");
    assert_debug_names_type::<state_v1::DestroyRequestedCredentialState>("DestroyRequestedCredentialState");
    assert_debug_names_type::<state_v1::DestroyedCredentialState>("DestroyedCredentialState");
    assert_debug_names_type::<state_v1::CleanupFailedCredentialState>("CleanupFailedCredentialState");
    assert_debug_names_type::<checkpoints_v1::RuntimeProjectionCheckpoint>("RuntimeProjectionCheckpoint");
    assert_debug_names_type::<checkpoints_v1::CredentialRecoveryWorkerCheckpoint>("CredentialRecoveryWorkerCheckpoint");
    assert_debug_names_type::<idempotency_v1::CredentialManagementIdempotencyRecord>(
        "CredentialManagementIdempotencyRecord",
    );
    assert_debug_names_type::<idempotency_v1::CredentialCommandResponse>("CredentialCommandResponse");
    assert_debug_names_type::<idempotency_v1::CredentialRefResponse>("CredentialRefResponse");
}

fn assert_type_url<M: MessageName>(name: &str) {
    assert_eq!(M::TYPE_URL, format!("type.googleapis.com/{}.{name}", M::PACKAGE));
    assert_eq!(M::FULL_NAME, format!("{}.{name}", M::PACKAGE));
}

#[test]
fn type_urls_use_the_declaring_package() {
    assert_type_url::<v1::CredentialIdentity>("CredentialIdentity");
    assert_type_url::<v1::CredentialKey>("CredentialKey");
    assert_type_url::<v1::CredentialEvent>("CredentialEvent");
    assert_type_url::<state_v1::ActiveCredentialState>("ActiveCredentialState");
    assert_type_url::<state_v1::CredentialStateSnapshot>("CredentialStateSnapshot");
    assert_type_url::<checkpoints_v1::RuntimeProjectionCheckpoint>("RuntimeProjectionCheckpoint");
    assert_type_url::<idempotency_v1::CredentialRefResponse>("CredentialRefResponse");
}

#[test]
fn builder_setters_populate_explicit_presence_fields() {
    let built = v1::CredentialIdentity {
        credential_id: CREDENTIAL_ID.into(),
        owner_id: "owner-1".into(),
        scope_key: "github".into(),
        ..Default::default()
    }
    .with_source(v1::CredentialSource::Github)
    .with_kind(v1::CredentialKind::WebhookSecret)
    .with_identity_version(v1::CredentialIdentityVersion::V1);
    assert_eq!(serde_json::to_value(&built).expect("identity"), identity());
    let credential_key = v1::CredentialKey {
        credential_id: CREDENTIAL_ID.into(),
        owner_id: "owner-1".into(),
    };

    let events = events();
    let activated = v1::CredentialActivated {
        key: credential_key.clone().into(),
        fingerprint: "sha256:abc".into(),
        ..Default::default()
    }
    .with_version(1);
    assert_eq!(
        serde_json::to_value(&activated).expect("activated"),
        payload(&events[2])
    );
    let rotated = v1::CredentialRotated {
        key: credential_key.into(),
        fingerprint: "sha256:def".into(),
        ..Default::default()
    }
    .with_previous_version(1)
    .with_version(2);
    assert_eq!(serde_json::to_value(&rotated).expect("rotated"), payload(&events[5]));
    let active = state_v1::ActiveCredentialState {
        identity: built.into(),
        fingerprint: "sha256:abc".into(),
        previous_versions: vec![1, 2],
        ..Default::default()
    }
    .with_version(3);
    assert_eq!(serde_json::to_value(&active).expect("active"), active_state());

    let projection = checkpoints_v1::RuntimeProjectionCheckpoint::default().with_last_scanned_sequence(9);
    assert_eq!(projection.last_scanned_sequence, Some(9));
    let worker = checkpoints_v1::CredentialRecoveryWorkerCheckpoint::default()
        .with_last_scanned_sequence(1)
        .with_consecutive_failure_count(2)
        .with_first_failure_unix_seconds(3)
        .with_retry_after_unix_seconds(4);
    assert_eq!(
        serde_json::to_value(&worker).expect("worker"),
        json!({
            "lastScannedSequence": "1", "consecutiveFailureCount": 2,
            "firstFailureUnixSeconds": "3", "retryAfterUnixSeconds": "4"
        })
    );

    let response = idempotency_v1::CredentialCommandResponse::default().with_stream_position(5);
    assert_eq!(response.stream_position, Some(5));
    let reference = idempotency_v1::CredentialRefResponse::default().with_version(6);
    assert_eq!(reference.version, Some(6));
    let record = idempotency_v1::CredentialManagementIdempotencyRecord::default()
        .with_status(idempotency_v1::CredentialManagementIdempotencyStatus::Completed);
    assert_eq!(
        record.status,
        Some(EnumValue::from(
            idempotency_v1::CredentialManagementIdempotencyStatus::Completed
        ))
    );
}

fn assert_registered<M: Message + MessageName + DeserializeOwned + Serialize>(
    registry: &buffa::type_registry::TypeRegistry,
    input: Value,
) {
    let expected: M = serde_json::from_value(input.clone()).expect("typed registry fixture");
    let entry = registry.json_any_by_url(M::TYPE_URL).expect("registered message URL");
    let wire = (entry.from_json)(input).expect("registered JSON codec");
    assert_eq!(wire, expected.encode_to_vec());
    assert_eq!(
        (entry.to_json)(&wire).expect("registered wire codec"),
        serde_json::to_value(&expected).expect("typed JSON")
    );
    assert!((entry.to_json)(b"\x00").is_err());
    assert!((entry.from_json)(json!(false)).is_err());
}

fn payload(value: &Value) -> Value {
    value
        .as_object()
        .expect("oneof")
        .values()
        .next()
        .expect("payload")
        .clone()
}

#[test]
fn package_registries_expose_every_message_type_url() {
    let mut registry = buffa::type_registry::TypeRegistry::new();
    v1::register_types(&mut registry);
    state_v1::register_types(&mut registry);
    checkpoints_v1::register_types(&mut registry);
    idempotency_v1::register_types(&mut registry);
    let events = events();
    assert_registered::<v1::CredentialIdentity>(&registry, identity());
    assert_registered::<v1::CredentialKey>(&registry, key());
    assert_registered::<v1::CredentialEvent>(&registry, events[0].clone());
    assert_registered::<v1::CredentialWriteRequested>(&registry, payload(&events[0]));
    assert_registered::<v1::CredentialWriteFailed>(&registry, payload(&events[1]));
    assert_registered::<v1::CredentialActivated>(&registry, payload(&events[2]));
    assert_registered::<v1::CredentialRotationRequested>(&registry, payload(&events[3]));
    assert_registered::<v1::CredentialRotationFailed>(&registry, payload(&events[4]));
    assert_registered::<v1::CredentialRotated>(&registry, payload(&events[5]));
    assert_registered::<v1::CredentialRevoked>(&registry, payload(&events[6]));
    assert_registered::<v1::CredentialDestroyRequested>(&registry, payload(&events[7]));
    assert_registered::<v1::CredentialDestroyed>(&registry, payload(&events[8]));
    assert_registered::<v1::CredentialDestroyFailed>(&registry, payload(&events[9]));
    let snapshots = snapshots();
    assert_registered::<state_v1::CredentialStateSnapshot>(&registry, snapshots[2].clone());
    assert_registered::<state_v1::CredentialMissingState>(&registry, json!({}));
    assert_registered::<state_v1::PendingCredentialWriteState>(&registry, payload(&snapshots[1]));
    assert_registered::<state_v1::ActiveCredentialState>(&registry, payload(&snapshots[2]));
    assert_registered::<state_v1::FailedCredentialWriteState>(&registry, payload(&snapshots[3]));
    assert_registered::<state_v1::RotationPendingCredentialState>(&registry, payload(&snapshots[4]));
    assert_registered::<state_v1::RevokedCredentialState>(&registry, payload(&snapshots[5]));
    assert_registered::<state_v1::DestroyRequestedCredentialState>(&registry, payload(&snapshots[6]));
    assert_registered::<state_v1::DestroyedCredentialState>(&registry, payload(&snapshots[7]));
    assert_registered::<state_v1::CleanupFailedCredentialState>(&registry, payload(&snapshots[8]));
    assert_registered::<checkpoints_v1::RuntimeProjectionCheckpoint>(&registry, json!({"lastScannedSequence": "5"}));
    assert_registered::<checkpoints_v1::CredentialRecoveryWorkerCheckpoint>(
        &registry,
        json!({"consecutiveFailureCount": 1}),
    );
    assert_registered::<idempotency_v1::CredentialManagementIdempotencyRecord>(
        &registry,
        json!({"fingerprint": "fp", "response": command_response()}),
    );
    assert_registered::<idempotency_v1::CredentialCommandResponse>(&registry, command_response());
    assert_registered::<idempotency_v1::CredentialRefResponse>(&registry, command_response()["credentialRef"].clone());
}

fn assert_enum_json<E>(valid_name: &str, valid_number: i32, unknown_number: i32)
where
    E: DeserializeOwned + Serialize + Debug + PartialEq + buffa::Enumeration + Default + Copy,
{
    let parsed: E = serde_json::from_value(json!(valid_name)).expect("enum name");
    assert_eq!(serde_json::to_value(parsed).expect("enum JSON"), json!(valid_name));
    let numeric: E = serde_json::from_value(json!(valid_number)).expect("enum number");
    assert_eq!(parsed, numeric);
    let signed: E = serde_json::from_str(&valid_number.to_string()).expect("signed number");
    assert_eq!(parsed, signed);
    let negative = serde_json::from_value::<E>(json!(-1_i64));
    assert!(negative.is_err());
    let null: E = serde_json::from_value(Value::Null).expect("null enum");
    assert_eq!(null, E::default());
    for rejected in [
        json!("NOT_A_VARIANT"),
        json!(unknown_number),
        json!(u64::from(u32::MAX) + 1),
        json!(i64::MIN),
        json!(true),
        json!([]),
    ] {
        assert!(serde_json::from_value::<E>(rejected.clone()).is_err(), "{rejected}");
    }
    assert!(serde_json::from_str::<E>("-4294967297").is_err());
    assert!(serde_json::from_str::<E>("4294967296").is_err());
}

#[test]
fn enum_json_accepts_names_numbers_and_null_and_rejects_everything_else() {
    assert_enum_json::<v1::CredentialSource>("CREDENTIAL_SOURCE_PLAID", 15, 99);
    assert_enum_json::<v1::CredentialKind>("CREDENTIAL_KIND_WEBHOOK_SECRET", 9, 99);
    assert_enum_json::<v1::CredentialIdentityVersion>("CREDENTIAL_IDENTITY_VERSION_V1", 1, 99);
    assert_enum_json::<idempotency_v1::CredentialManagementIdempotencyStatus>(
        "CREDENTIAL_MANAGEMENT_IDEMPOTENCY_STATUS_COMPLETED",
        2,
        99,
    );
}

#[test]
fn identity_json_normalizes_snake_case_aliases() {
    let parsed: v1::CredentialIdentity = serde_json::from_value(json!({
        "credential_id": CREDENTIAL_ID, "owner_id": "owner-1", "source": 2,
        "scope_key": "github", "kind": 9, "identity_version": 1
    }))
    .expect("aliases");
    assert_eq!(serde_json::to_value(parsed).expect("canonical"), identity());
    let parsed: v1::CredentialKey =
        serde_json::from_value(json!({"credential_id": CREDENTIAL_ID, "owner_id": "owner-1"})).expect("key aliases");
    assert_eq!(serde_json::to_value(parsed).expect("canonical key"), key());
    assert_eq!(
        identity_with(json!({})),
        json!({"credentialId": "", "ownerId": "", "scopeKey": ""})
    );
}

#[test]
fn enum_values_participate_in_repeated_proto_json() {
    super::assert_proto_sequence(
        vec![v1::CredentialSource::Github, v1::CredentialSource::Zoom],
        json!(["CREDENTIAL_SOURCE_GITHUB", "CREDENTIAL_SOURCE_ZOOM"]),
    );
    super::assert_proto_sequence(vec![v1::CredentialKind::AppToken], json!(["CREDENTIAL_KIND_APP_TOKEN"]));
    super::assert_proto_sequence(
        vec![v1::CredentialIdentityVersion::V1],
        json!(["CREDENTIAL_IDENTITY_VERSION_V1"]),
    );
    super::assert_proto_sequence(
        vec![idempotency_v1::CredentialManagementIdempotencyStatus::InProgress],
        json!(["CREDENTIAL_MANAGEMENT_IDEMPOTENCY_STATUS_IN_PROGRESS"]),
    );
}
