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
use trogonai_proto::gateway::credentials::v1 as proto;

use super::super::domain::credential_event::proto_event_type;
use super::super::domain::{
    CredentialEvent, CredentialEventPayloadError, CredentialFailureReason, CredentialFingerprint, CredentialId,
    CredentialKind, CredentialMetadata, CredentialOwnerId, CredentialRef, CredentialScope, CredentialStatus,
    CredentialVersion, SourceKind, StorageBackend,
};
use super::super::snapshot::CREDENTIAL_SNAPSHOT_POLICY;
use super::super::state::{CredentialDecideError, CredentialState, evolve, initial_state};
use super::{
    ActivateCredentialRotation, ActivateCredentialWrite, RecordCredentialRotationFailure, RecordCredentialWriteFailure,
    RequestCredentialRotation, RequestCredentialWrite, RevokeCredential,
};

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

impl StreamRead<str> for CredentialTestStore {
    type Error = CredentialTestStoreError;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        let start = match request.from {
            ReadFrom::Beginning => 1,
            ReadFrom::Position(position) => position.as_u64(),
        };
        let events = self.events.lock().unwrap();
        let stream_events = events
            .iter()
            .filter(|event| event.stream_id() == request.stream_id && event.stream_position.as_u64() >= start)
            .cloned()
            .collect();
        Ok(ReadStreamResponse {
            current_position: current_position(&events, request.stream_id),
            events: stream_events,
        })
    }
}

impl StreamAppend<str> for CredentialTestStore {
    type Error = CredentialTestStoreError;

    async fn append_stream(&self, request: AppendStreamRequest<'_, str>) -> Result<AppendStreamResponse, Self::Error> {
        let mut events = self.events.lock().unwrap();
        let current_position = current_position(&events, request.stream_id);
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

fn credential_id() -> CredentialId {
    CredentialId::new("openbao:tenant-1:github/primary:webhook_secret").unwrap()
}

fn owner_id() -> CredentialOwnerId {
    CredentialOwnerId::new("tenant-1").unwrap()
}

fn credential_ref(version: u64) -> CredentialRef {
    let scope = CredentialScope::integration(
        owner_id(),
        SourceKind::GitHub,
        crate::source_integration_id::SourceIntegrationId::new("primary").unwrap(),
    );
    CredentialRef::new(
        credential_id(),
        CredentialVersion::new(version).unwrap(),
        &scope,
        CredentialKind::WebhookSecret,
    )
}

fn metadata(version: u64) -> CredentialMetadata {
    let credential_ref = credential_ref(version);
    CredentialMetadata::new(
        credential_ref.clone(),
        CredentialStatus::Active,
        StorageBackend::OpenBao,
        CredentialFingerprint::new(format!("openbao:secret/metadata/{credential_ref}")).unwrap(),
    )
}

fn write_requested() -> CredentialEvent {
    CredentialEvent::WriteRequested {
        credential_id: credential_id(),
        owner_id: owner_id(),
        source: SourceKind::GitHub,
        kind: CredentialKind::WebhookSecret,
    }
}

fn activated(version: u64) -> CredentialEvent {
    CredentialEvent::Activated {
        metadata: metadata(version),
    }
}

fn rotation_requested(version: u64) -> CredentialEvent {
    CredentialEvent::RotationRequested {
        credential_ref: credential_ref(version),
    }
}

fn rotation_failed(version: u64) -> CredentialEvent {
    CredentialEvent::RotationFailed {
        credential_ref: credential_ref(version),
        reason: CredentialFailureReason::new("openbao rotate failed").unwrap(),
    }
}

fn rotated(previous_version: u64, next_version: u64) -> CredentialEvent {
    CredentialEvent::Rotated {
        previous_credential_ref: credential_ref(previous_version),
        metadata: metadata(next_version),
    }
}

fn revoked(version: u64) -> CredentialEvent {
    CredentialEvent::Revoked {
        credential_ref: credential_ref(version),
    }
}

fn request_write() -> RequestCredentialWrite {
    RequestCredentialWrite::new(
        credential_id(),
        owner_id(),
        SourceKind::GitHub,
        CredentialKind::WebhookSecret,
    )
}

fn rebuild_state_from_events(events: impl IntoIterator<Item = CredentialEvent>) -> CredentialState {
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
}

#[test]
fn state_snapshot_round_trips_active_state() {
    let state = rebuild_state_from_events([write_requested(), activated(1), rotation_requested(1), rotated(1, 2)]);

    let encoded = SnapshotPayloadEncode::encode(&state).unwrap();
    let decoded = <CredentialState as SnapshotPayloadDecode>::decode(SnapshotPayloadData::new(&encoded)).unwrap();

    assert_eq!(decoded, state);
}

#[test]
fn state_snapshot_round_trips_pending_write_state() {
    let state = rebuild_state_from_events([write_requested()]);

    let encoded = SnapshotPayloadEncode::encode(&state).unwrap();
    let decoded = <CredentialState as SnapshotPayloadDecode>::decode(SnapshotPayloadData::new(&encoded)).unwrap();

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
fn given_when_then_activates_pending_credential_write() {
    TestCase::<ActivateCredentialWrite>::new()
        .given([write_requested()])
        .when(ActivateCredentialWrite::new(metadata(1)))
        .then([activated(1)]);
}

#[test]
fn given_when_then_records_pending_write_failure() {
    let reason = CredentialFailureReason::new("openbao write failed").unwrap();

    TestCase::<RecordCredentialWriteFailure>::new()
        .given([write_requested()])
        .when(RecordCredentialWriteFailure::new(credential_id(), reason.clone()))
        .then([CredentialEvent::WriteFailed {
            credential_id: credential_id(),
            reason,
        }]);
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
        .when(RecordCredentialRotationFailure::new(credential_ref(1), reason.clone()))
        .then([CredentialEvent::RotationFailed {
            credential_ref: credential_ref(1),
            reason,
        }]);
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
fn rebuild_state_from_events_active_rotated_state() {
    let state = [write_requested(), activated(1), rotation_requested(1), rotated(1, 2)]
        .into_iter()
        .try_fold(initial_state(), |state, event| evolve(state, &event))
        .unwrap();

    let CredentialState::Active(active) = state else {
        panic!("expected active credential");
    };
    assert_eq!(active.credential_ref(), &credential_ref(2));
    assert_eq!(active.previous_versions(), &[credential_ref(1)]);
}

#[test]
fn event_codec_round_trips_all_events() {
    for event in [
        write_requested(),
        CredentialEvent::WriteFailed {
            credential_id: credential_id(),
            reason: CredentialFailureReason::new("openbao unavailable").unwrap(),
        },
        activated(1),
        rotation_requested(1),
        rotation_failed(1),
        rotated(1, 2),
        revoked(2),
    ] {
        let event_type = event.event_type().unwrap();
        let payload = event.encode().unwrap();
        let decoded = CredentialEvent::decode(EventData::new(event_type, &payload))
            .unwrap()
            .into_decoded();

        assert_eq!(decoded, Some(event));
    }
}

#[test]
fn event_codec_preserves_integration_scope_key() {
    let event = activated(1);
    let payload = event.encode().unwrap();
    let proto_event = proto::CredentialActivated::decode_from_slice(&payload).unwrap();
    let metadata = proto_event.metadata.as_option().unwrap();
    let reference = metadata.reference.as_option().unwrap();

    assert_eq!(reference.scope_key, "github/primary");

    let decoded = CredentialEvent::decode(EventData::new(event.event_type().unwrap(), &payload))
        .unwrap()
        .into_decoded()
        .unwrap();
    let CredentialEvent::Activated { metadata } = decoded else {
        panic!("expected activated event");
    };
    assert_eq!(metadata.reference().scope_key(), "github/primary");
}

#[test]
fn event_codec_skips_foreign_event_types() {
    let decoded = CredentialEvent::decode(EventData::new("foreign.event.v1", b"{}")).unwrap();

    assert_eq!(decoded, EventDecodeOutcome::Skipped);
}

#[test]
fn event_codec_rejects_invalid_persisted_fields() {
    let payload = proto::CredentialWriteRequested {
        credential_id: String::new(),
        owner_id: "tenant-1".to_string(),
        source: Some(proto::CredentialSource::CREDENTIAL_SOURCE_GITHUB.into()),
        kind: Some(proto::CredentialKind::CREDENTIAL_KIND_WEBHOOK_SECRET.into()),
    }
    .encode_to_vec();

    let error = CredentialEvent::decode(EventData::new(
        proto_event_type::<proto::CredentialWriteRequested>(),
        &payload,
    ))
    .unwrap_err();

    assert!(matches!(
        error,
        CredentialEventPayloadError::InvalidField {
            field: "credential_id",
            ..
        }
    ));
}

#[test]
fn event_codec_rejects_scope_key_that_does_not_match_source() {
    let payload = proto::CredentialRotationRequested {
        credential_ref: MessageField::some(proto::CredentialRef {
            id: "openbao:tenant-1:github/primary:webhook_secret".to_string(),
            version: Some(1),
            owner_id: "tenant-1".to_string(),
            source: Some(proto::CredentialSource::CREDENTIAL_SOURCE_GITHUB.into()),
            scope_key: "slack/primary".to_string(),
            kind: Some(proto::CredentialKind::CREDENTIAL_KIND_WEBHOOK_SECRET.into()),
        }),
    }
    .encode_to_vec();

    let error = CredentialEvent::decode(EventData::new(
        proto_event_type::<proto::CredentialRotationRequested>(),
        &payload,
    ))
    .unwrap_err();

    assert!(matches!(
        error,
        CredentialEventPayloadError::InvalidField {
            field: "credential_ref.scope_key",
            ..
        }
    ));
}

#[tokio::test]
async fn command_execution_persists_and_replays_events() {
    let store = CredentialTestStore::default();

    let request_result = CommandExecution::new(&store, &request_write()).execute().await.unwrap();
    assert_eq!(request_result.stream_position, position(1));
    assert_eq!(request_result.events.as_slice(), &[write_requested()]);
    assert!(matches!(request_result.state, CredentialState::PendingWrite(_)));
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

    let CredentialState::Active(active) = activation_result.state else {
        panic!("expected active credential");
    };
    assert_eq!(active.credential_ref(), &credential_ref(1));
}

#[tokio::test]
async fn command_execution_rejects_duplicate_write_with_no_stream_precondition() {
    let store = CredentialTestStore::default();

    CommandExecution::new(&store, &request_write()).execute().await.unwrap();
    let error = CommandExecution::new(&store, &request_write())
        .execute()
        .await
        .unwrap_err();

    assert!(matches!(error, CommandError::Append(CredentialTestStoreError)));
    assert_eq!(
        store.write_preconditions(),
        [StreamWritePrecondition::NoStream, StreamWritePrecondition::NoStream]
    );
    assert_eq!(store.events().len(), 1);
}
