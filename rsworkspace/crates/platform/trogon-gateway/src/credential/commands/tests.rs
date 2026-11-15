use std::sync::Mutex;

use buffa::{Message as _, MessageField};
use chrono::Utc;
use trogon_decider::testing::TestCase;
use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, CommandError, CommandExecution, CommandSnapshotPolicy, EventData,
    EventDecode, EventDecodeOutcome, EventEncode, EventType, ReadFrom, ReadStreamRequest, ReadStreamResponse,
    SnapshotPayloadData, SnapshotPayloadDecode, SnapshotPayloadEncode, StreamAppend, StreamEvent, StreamPosition,
    StreamRead, StreamWritePrecondition,
};
use trogonai_proto::gateway::credentials::{CredentialEventCase, CredentialStateSnapshotCase, state_v1, v1 as proto};

use super::domain::{
    CredentialDestroyReason, CredentialFailureReason, CredentialFingerprint, CredentialId, CredentialIdentity,
    CredentialIdentityVersion, CredentialKey, CredentialKind, CredentialMetadata, CredentialOwnerId, CredentialRef,
    CredentialScope, CredentialStatus, CredentialVersion, SourceKind,
};
use super::proto::{
    CredentialFieldInvalidError, CredentialProtoDecodeError, activated_to_proto, active_state_to_proto,
    decode_active_state, decode_pending_write_state, decode_write_requested, destroy_failed_to_proto,
    destroy_requested_to_proto, destroyed_to_proto, identity_to_proto, pending_write_to_proto_state, revoked_to_proto,
    rotated_to_proto, rotation_failed_to_proto, rotation_requested_to_proto, write_failed_to_proto,
    write_requested_to_proto,
};
use super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
use super::state::{CredentialDecideError, CredentialEvolveError, evolve, initial_state};
use super::{
    ActivateCredentialRotation, ActivateCredentialWrite, CompleteCredentialDestroy, RecordCredentialDestroyFailure,
    RecordCredentialRotationFailure, RecordCredentialWriteFailure, RequestCredentialDestroy, RequestCredentialRotation,
    RequestCredentialWrite, RevokeCredential,
};
use crate::source_integration_id::SourceIntegrationId;

#[derive(Debug, thiserror::Error)]
#[error("credential test stream store rejected the append")]
struct CredentialTestStoreError;

#[derive(Default)]
struct CredentialTestStore {
    events: Mutex<Vec<StreamEvent>>,
    write_preconditions: Mutex<Vec<StreamWritePrecondition>>,
}

impl CredentialTestStore {
    fn write_preconditions(&self) -> Vec<StreamWritePrecondition> {
        self.write_preconditions.lock().unwrap().clone()
    }

    fn events(&self) -> Vec<StreamEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl StreamRead<CredentialKey> for CredentialTestStore {
    type Error = CredentialTestStoreError;

    async fn read_stream(
        &self,
        request: ReadStreamRequest<'_, CredentialKey>,
    ) -> Result<ReadStreamResponse, Self::Error> {
        let start = match request.from {
            ReadFrom::Beginning => 1,
            ReadFrom::Position(position) => position.as_u64(),
        };
        let stream_id = request.stream_id.to_string();
        let events = self.events.lock().unwrap();
        let stream_events = events
            .iter()
            .filter(|event| event.stream_id() == stream_id && event.stream_position.as_u64() >= start)
            .cloned()
            .collect();
        Ok(ReadStreamResponse {
            current_position: current_position(&events, &stream_id),
            events: stream_events,
        })
    }
}

impl StreamAppend<CredentialKey> for CredentialTestStore {
    type Error = CredentialTestStoreError;

    async fn append_stream(
        &self,
        request: AppendStreamRequest<'_, CredentialKey>,
    ) -> Result<AppendStreamResponse, Self::Error> {
        let mut events = self.events.lock().unwrap();
        let current_position = current_position(&events, &request.stream_id.to_string());
        self.write_preconditions
            .lock()
            .unwrap()
            .push(request.stream_write_precondition);
        match request.stream_write_precondition {
            StreamWritePrecondition::Any => {}
            StreamWritePrecondition::StreamExists if current_position.is_some() => {}
            StreamWritePrecondition::NoStream if current_position.is_none() => {}
            StreamWritePrecondition::At(position) if current_position == Some(position) => {}
            _ => return Err(CredentialTestStoreError),
        }

        let mut last_position = current_position;
        for event in request.events {
            let stream_position = position(events.len() as u64 + 1);
            last_position = Some(stream_position);
            events.push(StreamEvent {
                stream_id: request.stream_id.to_string(),
                event,
                stream_position,
                recorded_at: Utc::now(),
            });
        }

        Ok(AppendStreamResponse {
            stream_position: last_position.expect("append request must contain events"),
        })
    }
}

fn current_position(events: &[StreamEvent], stream_id: &str) -> Option<StreamPosition> {
    events
        .iter()
        .filter(|event| event.stream_id() == stream_id)
        .map(|event| event.stream_position)
        .max()
}

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).unwrap()
}

fn stream_id_of(credential_id: CredentialId) -> CredentialKey {
    CredentialKey::new(owner_id(), credential_id)
}

fn identity() -> CredentialIdentity {
    CredentialIdentity::derive(scope(), CredentialKind::WebhookSecret)
}

fn credential_id() -> CredentialId {
    identity().id().clone()
}

/// The recorded id under the secondary integration: the same stream, another identity.
fn identity_of_another_slot() -> CredentialIdentity {
    CredentialIdentity::new(
        credential_id(),
        integration_scope("secondary"),
        CredentialKind::WebhookSecret,
        CredentialIdentityVersion::V1,
    )
}

/// The webhook secret of the secondary integration, a credential with a stream of its own.
fn other_identity() -> CredentialIdentity {
    CredentialIdentity::derive(integration_scope("secondary"), CredentialKind::WebhookSecret)
}

fn version(value: u64) -> CredentialVersion {
    CredentialVersion::new(value).unwrap()
}

fn owner_id() -> CredentialOwnerId {
    CredentialOwnerId::new("tenant-1").unwrap()
}

fn integration_scope(integration_id: &str) -> CredentialScope {
    CredentialScope::integration(
        owner_id(),
        SourceKind::GitHub,
        SourceIntegrationId::new(integration_id).unwrap(),
    )
}

fn scope() -> CredentialScope {
    integration_scope("primary")
}

fn credential_ref(value: u64) -> CredentialRef {
    CredentialRef::new(identity(), version(value))
}

fn active_metadata(credential_ref: CredentialRef) -> CredentialMetadata {
    let fingerprint = CredentialFingerprint::new(format!("openbao:secret/metadata/{credential_ref}")).unwrap();
    CredentialMetadata::new(credential_ref, CredentialStatus::Active, fingerprint)
}

fn metadata(version: u64) -> CredentialMetadata {
    active_metadata(credential_ref(version))
}

fn write_requested() -> proto::CredentialEvent {
    proto::CredentialEvent {
        event: Some(write_requested_to_proto(&identity()).into()),
    }
}

fn write_failed(reason: CredentialFailureReason) -> proto::CredentialEvent {
    proto::CredentialEvent {
        event: Some(write_failed_to_proto(&stream_id_of(credential_id()), &reason).into()),
    }
}

fn activated(version: u64) -> proto::CredentialEvent {
    proto::CredentialEvent {
        event: Some(activated_to_proto(&metadata(version)).into()),
    }
}

fn rotation_requested(version: u64) -> proto::CredentialEvent {
    proto::CredentialEvent {
        event: Some(rotation_requested_to_proto(&credential_ref(version)).into()),
    }
}

fn rotation_failed(version: u64) -> proto::CredentialEvent {
    proto::CredentialEvent {
        event: Some(
            rotation_failed_to_proto(
                &credential_ref(version),
                &CredentialFailureReason::new("openbao rotate failed").unwrap(),
            )
            .into(),
        ),
    }
}

fn rotated(previous_version: u64, next_version: u64) -> proto::CredentialEvent {
    proto::CredentialEvent {
        event: Some(rotated_to_proto(&credential_ref(previous_version), &metadata(next_version)).into()),
    }
}

fn revoked(version: u64) -> proto::CredentialEvent {
    proto::CredentialEvent {
        event: Some(revoked_to_proto(&credential_ref(version)).into()),
    }
}

fn destroy_reason() -> CredentialDestroyReason {
    CredentialDestroyReason::new("credential lifecycle cleanup").unwrap()
}

fn destroy_failure_reason() -> CredentialFailureReason {
    CredentialFailureReason::new("openbao destroy failed").unwrap()
}

fn destroy_requested(version: u64) -> proto::CredentialEvent {
    proto::CredentialEvent {
        event: Some(destroy_requested_to_proto(&credential_ref(version), &destroy_reason()).into()),
    }
}

fn destroyed(version: u64) -> proto::CredentialEvent {
    proto::CredentialEvent {
        event: Some(destroyed_to_proto(&credential_ref(version)).into()),
    }
}

fn destroy_failed(version: u64) -> proto::CredentialEvent {
    proto::CredentialEvent {
        event: Some(destroy_failed_to_proto(&credential_ref(version), &destroy_failure_reason()).into()),
    }
}

fn request_write() -> RequestCredentialWrite {
    RequestCredentialWrite::new(identity())
}

fn rebuild_state_from_events(
    events: impl IntoIterator<Item = proto::CredentialEvent>,
) -> state_v1::CredentialStateSnapshot {
    events
        .into_iter()
        .try_fold(initial_state(), |state, event| evolve(state, &event))
        .unwrap()
}

#[test]
fn snapshot_policy_uses_scheduler_frequency_pattern() {
    assert_eq!(CREDENTIAL_SNAPSHOT_POLICY.frequency().get(), 32);
    assert_eq!(
        <RequestCredentialWrite as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
        CREDENTIAL_SNAPSHOT_POLICY
    );
    assert_eq!(
        <ActivateCredentialWrite as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
        CREDENTIAL_SNAPSHOT_POLICY
    );
    assert_eq!(
        <RecordCredentialWriteFailure as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
        CREDENTIAL_SNAPSHOT_POLICY
    );
    assert_eq!(
        <RequestCredentialRotation as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
        CREDENTIAL_SNAPSHOT_POLICY
    );
    assert_eq!(
        <RecordCredentialRotationFailure as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
        CREDENTIAL_SNAPSHOT_POLICY
    );
    assert_eq!(
        <ActivateCredentialRotation as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
        CREDENTIAL_SNAPSHOT_POLICY
    );
    assert_eq!(
        <RevokeCredential as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
        CREDENTIAL_SNAPSHOT_POLICY
    );
    assert_eq!(
        <RequestCredentialDestroy as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
        CREDENTIAL_SNAPSHOT_POLICY
    );
    assert_eq!(
        <CompleteCredentialDestroy as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
        CREDENTIAL_SNAPSHOT_POLICY
    );
    assert_eq!(
        <RecordCredentialDestroyFailure as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
        CREDENTIAL_SNAPSHOT_POLICY
    );
}

#[test]
fn state_snapshot_round_trips_active_state() {
    let state = rebuild_state_from_events([write_requested(), activated(1), rotation_requested(1), rotated(1, 2)]);

    let encoded = SnapshotPayloadEncode::encode(&state).unwrap();
    let decoded =
        <state_v1::CredentialStateSnapshot as SnapshotPayloadDecode>::decode(SnapshotPayloadData::new(&encoded))
            .unwrap();

    assert_eq!(decoded, state);
}

#[test]
fn state_snapshot_round_trips_pending_write_state() {
    let state = rebuild_state_from_events([write_requested()]);

    let encoded = SnapshotPayloadEncode::encode(&state).unwrap();
    let decoded =
        <state_v1::CredentialStateSnapshot as SnapshotPayloadDecode>::decode(SnapshotPayloadData::new(&encoded))
            .unwrap();

    assert_eq!(decoded, state);
}

#[test]
fn state_snapshot_round_trips_destroyed_state() {
    let state = rebuild_state_from_events([
        write_requested(),
        activated(1),
        revoked(1),
        destroy_requested(1),
        destroyed(1),
    ]);

    let encoded = SnapshotPayloadEncode::encode(&state).unwrap();
    let decoded =
        <state_v1::CredentialStateSnapshot as SnapshotPayloadDecode>::decode(SnapshotPayloadData::new(&encoded))
            .unwrap();

    assert_eq!(decoded, state);
}

#[test]
fn state_snapshot_round_trips_cleanup_failed_state() {
    let state = rebuild_state_from_events([
        write_requested(),
        activated(1),
        revoked(1),
        destroy_requested(1),
        destroy_failed(1),
    ]);

    let encoded = SnapshotPayloadEncode::encode(&state).unwrap();
    let decoded =
        <state_v1::CredentialStateSnapshot as SnapshotPayloadDecode>::decode(SnapshotPayloadData::new(&encoded))
            .unwrap();

    assert_eq!(decoded, state);
}

#[test]
fn given_when_then_requests_credential_write() {
    TestCase::<RequestCredentialWrite>::new()
        .given_no_history()
        .when(request_write())
        .then([write_requested()]);
}

#[test]
fn given_when_then_rejects_duplicate_credential_write() {
    TestCase::<RequestCredentialWrite>::new()
        .given([write_requested()])
        .when(request_write())
        .then_error(CredentialDecideError::AlreadyExists {
            credential_id: credential_id(),
        });
}

#[test]
fn given_when_then_rejects_credential_write_for_active_credential() {
    TestCase::<RequestCredentialWrite>::new()
        .given([write_requested()])
        .given([activated(1)])
        .when(request_write())
        .then_error(CredentialDecideError::AlreadyExists {
            credential_id: credential_id(),
        });
}

#[test]
fn given_when_then_rejects_credential_write_for_revoked_credential() {
    TestCase::<RequestCredentialWrite>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([revoked(1)])
        .when(request_write())
        .then_error(CredentialDecideError::Revoked {
            credential_id: credential_id(),
        });
}

#[test]
fn given_when_then_allows_write_after_credential_destroyed() {
    TestCase::<RequestCredentialWrite>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([revoked(1)])
        .given([destroy_requested(1)])
        .given([destroyed(1)])
        .when(request_write())
        .then([write_requested()]);
}

#[test]
fn given_when_then_allows_write_retry_after_write_failure() {
    let reason = CredentialFailureReason::new("openbao write failed").unwrap();

    TestCase::<RequestCredentialWrite>::new()
        .given([write_requested()])
        .given([write_failed(reason)])
        .when(request_write())
        .then([write_requested()]);
}

#[test]
fn given_when_then_rejects_a_write_retry_under_another_identity() {
    let reason = CredentialFailureReason::new("openbao write failed").unwrap();

    TestCase::<RequestCredentialWrite>::new()
        .given([write_requested()])
        .given([write_failed(reason)])
        .when(RequestCredentialWrite::new(identity_of_another_slot()))
        .then_error(CredentialDecideError::IdentityMismatch {
            key: stream_id_of(credential_id()),
        });
}

#[test]
fn given_when_then_rejects_a_write_after_destroy_under_another_identity() {
    TestCase::<RequestCredentialWrite>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([revoked(1)])
        .given([destroy_requested(1)])
        .given([destroyed(1)])
        .when(RequestCredentialWrite::new(identity_of_another_slot()))
        .then_error(CredentialDecideError::IdentityMismatch {
            key: stream_id_of(credential_id()),
        });
}

#[test]
fn given_when_then_activates_pending_credential_write() {
    TestCase::<ActivateCredentialWrite>::new()
        .given([write_requested()])
        .when(ActivateCredentialWrite::new(metadata(1)))
        .then([activated(1)]);
}

#[test]
fn given_when_then_rejects_activation_for_a_different_scope() {
    let other_scope_ref = CredentialRef::new(identity_of_another_slot(), CredentialVersion::initial());

    TestCase::<ActivateCredentialWrite>::new()
        .given([write_requested()])
        .when(ActivateCredentialWrite::new(active_metadata(other_scope_ref)))
        .then_error(CredentialDecideError::IdentityMismatch {
            key: stream_id_of(credential_id()),
        });
}

#[test]
fn given_when_then_rejects_rotation_into_a_different_scope() {
    let other_scope_ref = CredentialRef::new(identity_of_another_slot(), version(2));

    TestCase::<ActivateCredentialRotation>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([rotation_requested(1)])
        .when(ActivateCredentialRotation::new(active_metadata(other_scope_ref)))
        .then_error(CredentialDecideError::IdentityMismatch {
            key: stream_id_of(credential_id()),
        });
}

#[test]
fn activation_rejects_a_pending_write_persisted_without_a_scope() {
    let mut pending = pending_write_to_proto_state(&identity());
    pending.identity.as_option_mut().unwrap().scope_key = String::new();
    let state = state_v1::CredentialStateSnapshot {
        state: Some(pending.into()),
    };

    TestCase::<ActivateCredentialWrite>::new()
        .given_state(state)
        .when(ActivateCredentialWrite::new(metadata(1)))
        .then_error(CredentialDecideError::InvalidProto(
            CredentialProtoDecodeError::InvalidField {
                field: "pending_write.identity.scope_key".into(),
                reason: CredentialFieldInvalidError::EmptyScopeKey,
            },
        ));
}

#[test]
fn given_when_then_records_pending_write_failure() {
    let reason = CredentialFailureReason::new("openbao write failed").unwrap();

    TestCase::<RecordCredentialWriteFailure>::new()
        .given([write_requested()])
        .when(RecordCredentialWriteFailure::new(
            stream_id_of(credential_id()),
            reason.clone(),
        ))
        .then([write_failed(reason)]);
}

#[test]
fn given_when_then_rejects_a_write_failure_for_another_credential() {
    TestCase::<RecordCredentialWriteFailure>::new()
        .given([write_requested()])
        .when(RecordCredentialWriteFailure::new(
            other_identity().key(),
            CredentialFailureReason::secret_store_write_failed(),
        ))
        .then_error(CredentialDecideError::IdentityMismatch {
            key: other_identity().key(),
        });
}

#[test]
fn given_when_then_requests_rotation_for_active_credential() {
    TestCase::<RequestCredentialRotation>::new()
        .given([write_requested()])
        .given([activated(1)])
        .when(RequestCredentialRotation::new(credential_ref(1)))
        .then([rotation_requested(1)]);
}

#[test]
fn given_when_then_rejects_a_rotation_request_for_a_version_that_is_not_active() {
    TestCase::<RequestCredentialRotation>::new()
        .given([write_requested()])
        .given([activated(1)])
        .when(RequestCredentialRotation::new(credential_ref(2)))
        .then_error(CredentialDecideError::VersionMismatch {
            expected: version(1),
            actual: version(2),
        });
}

#[test]
fn given_when_then_activates_pending_rotation() {
    TestCase::<ActivateCredentialRotation>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([rotation_requested(1)])
        .when(ActivateCredentialRotation::new(metadata(2)))
        .then([rotated(1, 2)]);
}

#[test]
fn given_when_then_records_pending_rotation_failure() {
    let reason = CredentialFailureReason::new("openbao rotate failed").unwrap();

    TestCase::<RecordCredentialRotationFailure>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([rotation_requested(1)])
        .when(RecordCredentialRotationFailure::new(credential_ref(1), reason))
        .then([rotation_failed(1)]);
}

#[test]
fn given_when_then_rejects_a_rotation_failure_for_another_version() {
    let reason = CredentialFailureReason::new("openbao rotate failed").unwrap();

    TestCase::<RecordCredentialRotationFailure>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([rotation_requested(1)])
        .when(RecordCredentialRotationFailure::new(credential_ref(2), reason))
        .then_error(CredentialDecideError::VersionMismatch {
            expected: version(1),
            actual: version(2),
        });
}

#[test]
fn given_when_then_rejects_duplicate_rotation_request() {
    TestCase::<RequestCredentialRotation>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([rotation_requested(1)])
        .when(RequestCredentialRotation::new(credential_ref(1)))
        .then_error(CredentialDecideError::CredentialRotationAlreadyPending {
            credential_id: credential_id(),
        });
}

#[test]
fn given_when_then_rejects_rotation_with_stale_version() {
    TestCase::<ActivateCredentialRotation>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([rotation_requested(1)])
        .when(ActivateCredentialRotation::new(metadata(1)))
        .then_error(CredentialDecideError::RotationVersionNotNewer);
}

#[test]
fn given_when_then_allows_rotation_retry_after_rotation_failure() {
    TestCase::<RequestCredentialRotation>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([rotation_requested(1)])
        .given([rotation_failed(1)])
        .when(RequestCredentialRotation::new(credential_ref(1)))
        .then([rotation_requested(1)]);
}

#[test]
fn given_when_then_revokes_active_credential() {
    TestCase::<RevokeCredential>::new()
        .given([write_requested()])
        .given([activated(1)])
        .when(RevokeCredential::new(credential_ref(1)))
        .then([revoked(1)]);
}

#[test]
fn given_when_then_revokes_a_credential_with_a_pending_rotation() {
    TestCase::<RevokeCredential>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([rotation_requested(1)])
        .when(RevokeCredential::new(credential_ref(1)))
        .then([revoked(1)]);
}

#[test]
fn given_when_then_rejects_revoking_a_version_that_was_rotated_out() {
    TestCase::<RevokeCredential>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([rotation_requested(1)])
        .given([rotated(1, 2)])
        .when(RevokeCredential::new(credential_ref(1)))
        .then_error(CredentialDecideError::VersionMismatch {
            expected: version(2),
            actual: version(1),
        });
}

#[test]
fn given_when_then_rejects_revoking_a_credential_that_is_not_active() {
    TestCase::<RevokeCredential>::new()
        .given([write_requested()])
        .when(RevokeCredential::new(credential_ref(1)))
        .then_error(CredentialDecideError::CredentialNotActive {
            credential_id: credential_id(),
        });
}

#[test]
fn given_when_then_requests_destroy_for_revoked_credential() {
    TestCase::<RequestCredentialDestroy>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([revoked(1)])
        .when(RequestCredentialDestroy::new(credential_ref(1), destroy_reason()))
        .then([destroy_requested(1)]);
}

#[test]
fn given_when_then_rejects_a_destroy_request_under_another_identity() {
    TestCase::<RequestCredentialDestroy>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([revoked(1)])
        .when(RequestCredentialDestroy::new(
            CredentialRef::new(identity_of_another_slot(), version(1)),
            destroy_reason(),
        ))
        .then_error(CredentialDecideError::IdentityMismatch {
            key: stream_id_of(credential_id()),
        });
}

#[test]
fn given_when_then_rejects_destroy_for_active_credential() {
    TestCase::<RequestCredentialDestroy>::new()
        .given([write_requested()])
        .given([activated(1)])
        .when(RequestCredentialDestroy::new(credential_ref(1), destroy_reason()))
        .then_error(CredentialDecideError::CredentialNotRevokedOrExpired {
            credential_id: credential_id(),
        });
}

#[test]
fn given_when_then_rejects_destroy_for_pending_credential() {
    TestCase::<RequestCredentialDestroy>::new()
        .given([write_requested()])
        .when(RequestCredentialDestroy::new(credential_ref(1), destroy_reason()))
        .then_error(CredentialDecideError::CredentialNotRevokedOrExpired {
            credential_id: credential_id(),
        });
}

#[test]
fn given_when_then_rejects_duplicate_destroy_request() {
    TestCase::<RequestCredentialDestroy>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([revoked(1)])
        .given([destroy_requested(1)])
        .when(RequestCredentialDestroy::new(credential_ref(1), destroy_reason()))
        .then_error(CredentialDecideError::CredentialDestroyAlreadyRequested {
            credential_id: credential_id(),
        });
}

#[test]
fn given_when_then_rejects_destroy_when_already_destroyed() {
    TestCase::<RequestCredentialDestroy>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([revoked(1)])
        .given([destroy_requested(1)])
        .given([destroyed(1)])
        .when(RequestCredentialDestroy::new(credential_ref(1), destroy_reason()))
        .then_error(CredentialDecideError::CredentialAlreadyDestroyed {
            credential_id: credential_id(),
        });
}

#[test]
fn given_when_then_allows_destroy_retry_after_cleanup_failure() {
    TestCase::<RequestCredentialDestroy>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([revoked(1)])
        .given([destroy_requested(1)])
        .given([destroy_failed(1)])
        .when(RequestCredentialDestroy::new(credential_ref(1), destroy_reason()))
        .then([destroy_requested(1)]);
}

#[test]
fn given_when_then_completes_destroy_for_pending_destroy_request() {
    TestCase::<CompleteCredentialDestroy>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([revoked(1)])
        .given([destroy_requested(1)])
        .when(CompleteCredentialDestroy::new(credential_ref(1)))
        .then([destroyed(1)]);
}

#[test]
fn given_when_then_rejects_completing_the_destroy_of_another_version() {
    TestCase::<CompleteCredentialDestroy>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([revoked(1)])
        .given([destroy_requested(1)])
        .when(CompleteCredentialDestroy::new(credential_ref(2)))
        .then_error(CredentialDecideError::VersionMismatch {
            expected: version(1),
            actual: version(2),
        });
}

#[test]
fn given_when_then_rejects_completion_without_pending_destroy_request() {
    TestCase::<CompleteCredentialDestroy>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([revoked(1)])
        .when(CompleteCredentialDestroy::new(credential_ref(1)))
        .then_error(CredentialDecideError::CredentialDestroyNotPending {
            credential_id: credential_id(),
        });
}

#[test]
fn given_when_then_records_pending_destroy_failure() {
    TestCase::<RecordCredentialDestroyFailure>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([revoked(1)])
        .given([destroy_requested(1)])
        .when(RecordCredentialDestroyFailure::new(
            credential_ref(1),
            destroy_failure_reason(),
        ))
        .then([destroy_failed(1)]);
}

#[test]
fn given_when_then_rejects_a_destroy_failure_for_another_version() {
    TestCase::<RecordCredentialDestroyFailure>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([revoked(1)])
        .given([destroy_requested(1)])
        .when(RecordCredentialDestroyFailure::new(
            credential_ref(2),
            destroy_failure_reason(),
        ))
        .then_error(CredentialDecideError::VersionMismatch {
            expected: version(1),
            actual: version(2),
        });
}

#[test]
fn given_when_then_rejects_destroy_failure_without_pending_destroy_request() {
    TestCase::<RecordCredentialDestroyFailure>::new()
        .given([write_requested()])
        .given([activated(1)])
        .given([revoked(1)])
        .when(RecordCredentialDestroyFailure::new(
            credential_ref(1),
            destroy_failure_reason(),
        ))
        .then_error(CredentialDecideError::CredentialDestroyNotPending {
            credential_id: credential_id(),
        });
}

#[test]
fn given_when_then_rejects_write_activation_without_a_pending_write() {
    TestCase::<ActivateCredentialWrite>::new()
        .given_no_history()
        .when(ActivateCredentialWrite::new(metadata(1)))
        .then_error(CredentialDecideError::CredentialWriteNotPending {
            credential_id: credential_id(),
        });
}

#[test]
fn given_when_then_rejects_write_failure_without_a_pending_write() {
    TestCase::<RecordCredentialWriteFailure>::new()
        .given([write_requested()])
        .given([activated(1)])
        .when(RecordCredentialWriteFailure::new(
            stream_id_of(credential_id()),
            CredentialFailureReason::secret_store_write_failed(),
        ))
        .then_error(CredentialDecideError::CredentialWriteNotPending {
            credential_id: credential_id(),
        });
}

#[test]
fn given_when_then_rejects_rotation_request_for_a_credential_that_is_not_active() {
    TestCase::<RequestCredentialRotation>::new()
        .given([write_requested()])
        .when(RequestCredentialRotation::new(credential_ref(1)))
        .then_error(CredentialDecideError::CredentialNotActive {
            credential_id: credential_id(),
        });
}

#[test]
fn given_when_then_rejects_rotation_activation_without_a_pending_rotation() {
    TestCase::<ActivateCredentialRotation>::new()
        .given([write_requested()])
        .given([activated(1)])
        .when(ActivateCredentialRotation::new(metadata(2)))
        .then_error(CredentialDecideError::CredentialRotationNotPending {
            credential_id: credential_id(),
        });
}

#[test]
fn given_when_then_rejects_rotation_failure_without_a_pending_rotation() {
    TestCase::<RecordCredentialRotationFailure>::new()
        .given([write_requested()])
        .given([activated(1)])
        .when(RecordCredentialRotationFailure::new(
            credential_ref(1),
            CredentialFailureReason::new("openbao rotate failed").unwrap(),
        ))
        .then_error(CredentialDecideError::CredentialRotationNotPending {
            credential_id: credential_id(),
        });
}

#[test]
fn rebuild_state_from_events_active_rotated_state() {
    let state = [write_requested(), activated(1), rotation_requested(1), rotated(1, 2)]
        .into_iter()
        .try_fold(initial_state(), |state, event| evolve(state, &event))
        .unwrap();

    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (metadata, previous_versions) = decode_active_state("active", active).unwrap();
    assert_eq!(metadata.reference(), &credential_ref(2));
    assert_eq!(previous_versions, vec![credential_ref(1)]);
}

#[test]
fn rebuild_state_from_events_returns_failed_write_to_pending_write_on_retry() {
    let reason = CredentialFailureReason::new("openbao write failed").unwrap();

    let state = rebuild_state_from_events([write_requested(), write_failed(reason), write_requested()]);

    let Some(CredentialStateSnapshotCase::PendingWrite(pending)) = state.state.as_ref() else {
        panic!("expected pending write");
    };
    assert_eq!(decode_pending_write_state(pending).unwrap(), identity());
}

#[test]
fn rebuild_state_from_events_restarts_a_destroyed_credential_as_pending_write() {
    let state = rebuild_state_from_events([
        write_requested(),
        activated(1),
        revoked(1),
        destroy_requested(1),
        destroyed(1),
        write_requested(),
    ]);

    let Some(CredentialStateSnapshotCase::PendingWrite(pending)) = state.state.as_ref() else {
        panic!("expected pending write");
    };
    assert_eq!(decode_pending_write_state(pending).unwrap(), identity());
}

#[test]
fn evolve_rejects_restart_after_destroy_for_a_different_credential() {
    let destroyed_state = rebuild_state_from_events([
        write_requested(),
        activated(1),
        revoked(1),
        destroy_requested(1),
        destroyed(1),
    ]);
    let restart = proto::CredentialEvent {
        event: Some(write_requested_to_proto(&other_identity()).into()),
    };

    assert_eq!(
        evolve(destroyed_state, &restart).unwrap_err(),
        CredentialEvolveError::IdentityMismatch {
            key: stream_id_of(credential_id()),
        }
    );
}

#[test]
fn pending_write_keeps_the_requested_scope() {
    let state = rebuild_state_from_events([write_requested()]);

    let Some(CredentialStateSnapshotCase::PendingWrite(pending)) = state.state.as_ref() else {
        panic!("expected pending write");
    };
    assert_eq!(pending.identity.as_option().unwrap().scope_key, "github/primary");
    assert_eq!(decode_pending_write_state(pending).unwrap(), identity());
}

#[test]
fn write_requested_round_trips_a_source_scope() {
    let identity = CredentialIdentity::derive(
        CredentialScope::source(owner_id(), SourceKind::Discord),
        CredentialKind::BotToken,
    );

    let event = write_requested_to_proto(&identity);

    assert_eq!(event.identity.as_option().unwrap().scope_key, "discord");
    assert_eq!(decode_write_requested(&event).unwrap(), identity);
}

#[test]
fn evolve_rejects_write_retry_for_a_different_credential() {
    let reason = CredentialFailureReason::new("openbao write failed").unwrap();
    let failed = rebuild_state_from_events([write_requested(), write_failed(reason)]);
    let retry = proto::CredentialEvent {
        event: Some(write_requested_to_proto(&other_identity()).into()),
    };

    assert_eq!(
        evolve(failed, &retry).unwrap_err(),
        CredentialEvolveError::IdentityMismatch {
            key: stream_id_of(credential_id()),
        }
    );
}

#[test]
fn event_codec_round_trips_all_events() {
    for event in [
        write_requested(),
        write_failed(CredentialFailureReason::new("openbao unavailable").unwrap()),
        activated(1),
        rotation_requested(1),
        rotation_failed(1),
        rotated(1, 2),
        revoked(2),
        destroy_requested(2),
        destroyed(2),
        destroy_failed(2),
    ] {
        let event_type = event.event_type().unwrap();
        let payload = EventEncode::encode(&event).unwrap();
        let decoded = <proto::CredentialEvent as EventDecode>::decode(EventData::new(event_type, &payload))
            .unwrap()
            .into_decoded();

        assert_eq!(decoded, Some(event));
    }
}

#[test]
fn event_codec_preserves_integration_scope_key() {
    let event = write_requested();
    let payload = EventEncode::encode(&event).unwrap();
    let proto_event = proto::CredentialWriteRequested::decode_from_slice(&payload).unwrap();

    assert_eq!(proto_event.identity.as_option().unwrap().scope_key, "github/primary");

    let decoded =
        <proto::CredentialEvent as EventDecode>::decode(EventData::new(event.event_type().unwrap(), &payload))
            .unwrap()
            .into_decoded()
            .unwrap();
    let Some(CredentialEventCase::WriteRequested(requested)) = decoded.event else {
        panic!("expected write requested event");
    };
    assert_eq!(
        decode_write_requested(&requested).unwrap().scope_key(),
        "github/primary"
    );
}

#[test]
fn event_codec_skips_foreign_event_types() {
    let decoded = <proto::CredentialEvent as EventDecode>::decode(EventData::new("foreign.event.v1", b"{}")).unwrap();

    assert_eq!(decoded, EventDecodeOutcome::Skipped);
}

#[test]
fn event_codec_rejects_invalid_persisted_fields() {
    let payload = proto::CredentialWriteRequested {
        identity: MessageField::some(proto::CredentialIdentity {
            credential_id: String::new(),
            ..identity_to_proto(&identity())
        }),
    }
    .encode_to_vec();

    let decoded = <proto::CredentialEvent as EventDecode>::decode(EventData::new(
        <proto::CredentialWriteRequested as buffa::MessageName>::FULL_NAME,
        &payload,
    ))
    .unwrap()
    .into_decoded()
    .unwrap();

    let error = evolve(initial_state(), &decoded).unwrap_err();

    assert!(matches!(
        error,
        CredentialEvolveError::InvalidProto(CredentialProtoDecodeError::InvalidField {
            field,
            ..
        }) if field == "identity.credential_id"
    ));
}

#[test]
fn event_codec_rejects_a_write_request_without_a_scope() {
    let payload = proto::CredentialWriteRequested {
        identity: MessageField::some(proto::CredentialIdentity {
            scope_key: String::new(),
            ..identity_to_proto(&identity())
        }),
    }
    .encode_to_vec();

    let decoded = <proto::CredentialEvent as EventDecode>::decode(EventData::new(
        <proto::CredentialWriteRequested as buffa::MessageName>::FULL_NAME,
        &payload,
    ))
    .unwrap()
    .into_decoded()
    .unwrap();

    assert_eq!(
        evolve(initial_state(), &decoded).unwrap_err(),
        CredentialEvolveError::InvalidProto(CredentialProtoDecodeError::InvalidField {
            field: "identity.scope_key".into(),
            reason: CredentialFieldInvalidError::EmptyScopeKey,
        })
    );
}

#[test]
fn event_codec_rejects_scope_key_that_does_not_match_source() {
    let payload = proto::CredentialWriteRequested {
        identity: MessageField::some(proto::CredentialIdentity {
            scope_key: "slack/primary".to_string(),
            ..identity_to_proto(&identity())
        }),
    }
    .encode_to_vec();

    let decoded = <proto::CredentialEvent as EventDecode>::decode(EventData::new(
        <proto::CredentialWriteRequested as buffa::MessageName>::FULL_NAME,
        &payload,
    ))
    .unwrap()
    .into_decoded()
    .unwrap();

    let error = evolve(initial_state(), &decoded).unwrap_err();

    assert!(matches!(
        error,
        CredentialEvolveError::InvalidProto(CredentialProtoDecodeError::InvalidField {
            field,
            reason: CredentialFieldInvalidError::ScopeKeyMismatch { .. },
        }) if field == "identity.scope_key"
    ));
}

#[test]
fn active_state_decode_errors_name_the_full_field_path() {
    let metadata = metadata(1);
    let mut active = active_state_to_proto(metadata.reference(), metadata.fingerprint(), &[]);
    active.identity.as_option_mut().unwrap().credential_id = String::new();

    let error = decode_active_state("active", &active).unwrap_err();

    assert!(matches!(
        error,
        CredentialProtoDecodeError::InvalidField { field, .. } if field == "active.identity.credential_id"
    ));
}

#[test]
fn previous_version_decode_errors_name_the_full_field_path() {
    let metadata = metadata(2);
    let active = state_v1::ActiveCredentialState {
        previous_versions: vec![0],
        ..active_state_to_proto(metadata.reference(), metadata.fingerprint(), &[])
    };

    let error = decode_active_state("active", &active).unwrap_err();

    assert!(matches!(
        error,
        CredentialProtoDecodeError::InvalidField { field, .. } if field == "active.previous_versions"
    ));
}

#[tokio::test]
async fn command_execution_persists_and_replays_events() {
    let store = CredentialTestStore::default();

    let request_result = CommandExecution::new(&store, &request_write()).execute().await.unwrap();
    assert_eq!(request_result.stream_position, position(1));
    assert_eq!(request_result.events.as_slice(), &[write_requested()]);
    assert!(matches!(
        request_result.state.state,
        Some(CredentialStateSnapshotCase::PendingWrite(_))
    ));
    assert_eq!(store.write_preconditions(), [StreamWritePrecondition::NoStream]);

    let activation = ActivateCredentialWrite::new(metadata(1));
    let activation_result = CommandExecution::new(&store, &activation).execute().await.unwrap();
    assert_eq!(activation_result.stream_position, position(2));
    assert_eq!(activation_result.events.as_slice(), &[activated(1)]);
    assert_eq!(
        store.write_preconditions(),
        [
            StreamWritePrecondition::NoStream,
            StreamWritePrecondition::At(position(1))
        ]
    );

    let Some(CredentialStateSnapshotCase::Active(active)) = activation_result.state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (metadata, _previous_versions) = decode_active_state("active", active).unwrap();
    assert_eq!(metadata.reference(), &credential_ref(1));
}

#[tokio::test]
async fn command_execution_rejects_duplicate_write_without_appending() {
    let store = CredentialTestStore::default();

    CommandExecution::new(&store, &request_write()).execute().await.unwrap();
    let error = CommandExecution::new(&store, &request_write())
        .execute()
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        CommandError::Decide(CredentialDecideError::AlreadyExists { .. })
    ));
    assert_eq!(store.write_preconditions(), [StreamWritePrecondition::NoStream]);
    assert_eq!(store.events().len(), 1);
}

struct ConcurrentReadStore {
    inner: CredentialTestStore,
    reads: tokio::sync::Barrier,
}

impl StreamRead<CredentialKey> for ConcurrentReadStore {
    type Error = CredentialTestStoreError;

    async fn read_stream(
        &self,
        request: ReadStreamRequest<'_, CredentialKey>,
    ) -> Result<ReadStreamResponse, Self::Error> {
        let response = self.inner.read_stream(request).await?;
        self.reads.wait().await;
        Ok(response)
    }
}

impl StreamAppend<CredentialKey> for ConcurrentReadStore {
    type Error = CredentialTestStoreError;

    async fn append_stream(
        &self,
        request: AppendStreamRequest<'_, CredentialKey>,
    ) -> Result<AppendStreamResponse, Self::Error> {
        self.inner.append_stream(request).await
    }
}

#[tokio::test]
async fn command_execution_lets_only_one_concurrent_first_write_succeed() {
    let store = ConcurrentReadStore {
        inner: CredentialTestStore::default(),
        reads: tokio::sync::Barrier::new(2),
    };
    let request = request_write();

    let (first, second) = tokio::join!(
        CommandExecution::new(&store, &request).execute(),
        CommandExecution::new(&store, &request).execute()
    );

    let outcomes = [first, second];
    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    assert!(
        outcomes
            .iter()
            .any(|outcome| matches!(outcome, Err(CommandError::Append(CredentialTestStoreError))))
    );
    assert_eq!(
        store.inner.write_preconditions(),
        [StreamWritePrecondition::NoStream, StreamWritePrecondition::NoStream]
    );
    assert_eq!(store.inner.events().len(), 1);
}
