use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use async_nats::HeaderMap;
use async_nats::header::NATS_MESSAGE_ID;
use async_nats::jetstream::message::StreamMessage;
use bytes::Bytes;
use chrono::Utc;
use time::OffsetDateTime;
use trogon_decider_nats::TROGON_EVENT_TYPE;
use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, Event, EventEncode, EventId, EventType, Headers, ReadFrom,
    ReadSnapshotRequest, ReadSnapshotResponse, ReadStreamRequest, ReadStreamResponse, Snapshot, SnapshotRead,
    SnapshotWrite, StreamPosition, StreamWritePrecondition, WriteSnapshotRequest, WriteSnapshotResponse,
};
use trogon_nats::jetstream::{
    JetStreamGetStream, MockJetStreamConsumerFactory, MockJetStreamKvClient, MockJetStreamKvStore,
};
use trogon_std::SecretString;
use trogon_std::log_capture::{CapturedEvents, LevelFilter};
use uuid::Uuid;

use super::*;
use crate::credential::commands::domain::{
    CredentialDestroyReason, CredentialFailureReason, CredentialFingerprint, CredentialMetadata, CredentialStatus,
    StorageBackend,
};
use crate::credential::commands::domain::{
    CredentialKind, CredentialOwnerId, CredentialRef, CredentialScope, CredentialVersion, SourceKind,
};
use crate::credential::commands::proto::{
    activated_to_proto, decode_active_state, destroy_failed_to_proto, destroy_requested_to_proto, destroyed_to_proto,
    revoked_to_proto, rotated_to_proto, rotation_failed_to_proto, rotation_requested_to_proto, write_failed_to_proto,
    write_requested_to_proto,
};
use crate::credential_lifecycle::handler::{
    CredentialHandler, CredentialHandlerError, DestroyStoredCredential, PutCredential, RevokeStoredCredential,
    RotateCredential,
};
use crate::credential_runtime_projection::{RuntimeCredentialRegistry, RuntimeIntegrationKey};
use crate::secret_store::SecretDestroyReason;
use crate::secret_store::{MockOpenBaoSecretStore, SecretStoreError, SecretStoreMetadata};
use crate::source_integration_id::SourceIntegrationId;
use trogonai_proto::gateway::credentials::CredentialStateSnapshotCase;

#[derive(Debug, thiserror::Error)]
#[error("worker test stream store rejected the request")]
struct WorkerTestStreamStoreError;

#[derive(Clone, Default)]
struct WorkerTestStreamStore {
    events: Arc<Mutex<Vec<StreamEvent>>>,
    snapshots: Arc<Mutex<BTreeMap<String, Snapshot<state_v1::CredentialStateSnapshot>>>>,
    write_preconditions: Arc<Mutex<Vec<StreamWritePrecondition>>>,
    fail_append_at: Arc<Mutex<Option<usize>>>,
    fail_read_stream_id: Arc<Mutex<Option<String>>>,
}

impl WorkerTestStreamStore {
    fn fail_append_at(&self, append_count: usize) {
        *self.fail_append_at.lock().unwrap() = Some(append_count);
    }

    fn fail_read_stream(&self, stream_id: &str) {
        *self.fail_read_stream_id.lock().unwrap() = Some(stream_id.to_string());
    }

    fn events_as_raw_stream_scan(&self) -> Vec<StreamEvent> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .map(|mut event| {
                event.stream_id = "gateway.credentials.events.v1.subject".to_string();
                event
            })
            .collect()
    }

    fn push_credential_event(&self, stream_id: &str, event: v1::CredentialEvent) {
        let mut events = self.events.lock().unwrap();
        let stream_position = position(events.len() as u64 + 1);
        events.push(StreamEvent {
            stream_id: stream_id.to_string(),
            event: runtime_event(stream_position.as_u64(), event),
            stream_position,
            recorded_at: Utc::now(),
        });
    }
}

impl StreamRead<str> for WorkerTestStreamStore {
    type Error = WorkerTestStreamStoreError;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        if self.fail_read_stream_id.lock().unwrap().as_deref() == Some(request.stream_id) {
            return Err(WorkerTestStreamStoreError);
        }
        let start = match request.from {
            ReadFrom::Beginning => 1,
            ReadFrom::Position(position) => position.as_u64(),
        };
        let events = self.events.lock().unwrap();
        Ok(ReadStreamResponse {
            current_position: current_position(&events, request.stream_id),
            events: events
                .iter()
                .filter(|event| event.stream_id() == request.stream_id && event.stream_position.as_u64() >= start)
                .cloned()
                .collect(),
        })
    }
}

impl StreamAppend<str> for WorkerTestStreamStore {
    type Error = WorkerTestStreamStoreError;

    async fn append_stream(&self, request: AppendStreamRequest<'_, str>) -> Result<AppendStreamResponse, Self::Error> {
        let mut events = self.events.lock().unwrap();
        let append_count = self.write_preconditions.lock().unwrap().len() + 1;
        self.write_preconditions
            .lock()
            .unwrap()
            .push(request.stream_write_precondition);
        {
            let mut fail_append_at = self.fail_append_at.lock().unwrap();
            if *fail_append_at == Some(append_count) {
                *fail_append_at = None;
                return Err(WorkerTestStreamStoreError);
            }
        }

        let current_position = current_position(&events, request.stream_id);
        match request.stream_write_precondition {
            StreamWritePrecondition::Any => {}
            StreamWritePrecondition::StreamExists if current_position.is_some() => {}
            StreamWritePrecondition::NoStream if current_position.is_none() => {}
            StreamWritePrecondition::At(position) if current_position == Some(position) => {}
            _ => return Err(WorkerTestStreamStoreError),
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

impl SnapshotRead<state_v1::CredentialStateSnapshot, str> for WorkerTestStreamStore {
    type Error = WorkerTestStreamStoreError;

    async fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, str>,
    ) -> Result<ReadSnapshotResponse<state_v1::CredentialStateSnapshot>, Self::Error> {
        Ok(ReadSnapshotResponse {
            snapshot: self.snapshots.lock().unwrap().get(request.snapshot_id).cloned(),
        })
    }
}

impl SnapshotWrite<state_v1::CredentialStateSnapshot, str> for WorkerTestStreamStore {
    type Error = WorkerTestStreamStoreError;

    async fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, state_v1::CredentialStateSnapshot, str>,
    ) -> Result<WriteSnapshotResponse, Self::Error> {
        self.snapshots
            .lock()
            .unwrap()
            .insert(request.snapshot_id.to_string(), request.snapshot);
        Ok(WriteSnapshotResponse)
    }
}

#[derive(Clone)]
struct UnavailableMetadataSecretStore;

impl SecretStoreMetadata for UnavailableMetadataSecretStore {
    type Error = SecretStoreError;

    async fn metadata(&self, _credential: &CredentialRef) -> Result<CredentialMetadata, Self::Error> {
        Err(SecretStoreError::BackendUnavailable {
            backend: StorageBackend::OpenBao,
            message: "metadata unavailable".to_string(),
        })
    }
}

#[tokio::test]
async fn recovery_plan_groups_by_payload_credential_id_instead_of_raw_subject() {
    let store = WorkerTestStreamStore::default();
    store.push_credential_event(credential_id().as_str(), write_requested_event());
    let events = store.events_as_raw_stream_scan();

    let plan = recovery_plan_from_credential_events(events, &store).await;

    assert_eq!(plan.report.scanned_events(), 1);
    assert_eq!(plan.report.decoded_events(), 1);
    assert_eq!(plan.report.planned_recoveries(), 1);
    assert_eq!(
        plan.commands,
        vec![PlannedCredentialRecovery {
            credential_id: credential_id(),
            command: CredentialActivationRecoveryCommand::Write(
                crate::credential_lifecycle::handler::RecoverCredentialWriteActivation::new(credential_ref(1))
            ),
        }]
    );
}

#[tokio::test]
async fn worker_recovers_pending_write_activation_after_secret_store_success() {
    let events = WorkerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    events.fail_append_at(2);
    let error = handler.put(put_command("super-secret")).await.unwrap_err();
    assert!(matches!(error, CredentialHandlerError::ActivateWrite { .. }));

    let runtime_credentials = RuntimeCredentialRegistry::default();
    let runtime_handler = CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials.clone());
    let report = recover_pending_credential_activations_from_events(
        events.events_as_raw_stream_scan(),
        &events,
        &runtime_handler,
    )
    .await;

    assert_eq!(report.planned_recoveries(), 1);
    assert_eq!(report.recovered_writes(), 1);
    assert_eq!(report.failed_recoveries(), 0);
    assert_eq!(
        runtime_credentials
            .resolver(secrets)
            .resolve(&runtime_key(), CredentialKind::WebhookSecret)
            .await
            .unwrap()
            .as_plaintext()
            .unwrap()
            .as_str(),
        "super-secret"
    );
}

#[tokio::test]
async fn worker_recovers_pending_rotation_activation_after_secret_store_success() {
    let events = WorkerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    let active = handler.put(put_command("old-secret")).await.unwrap();
    let active = active.into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = active.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let active_ref = active_metadata.reference().clone();
    events.fail_append_at(4);
    let error = handler
        .rotate(RotateCredential::new(
            active_ref,
            SecretString::new("new-secret").unwrap(),
        ))
        .await
        .unwrap_err();
    assert!(matches!(error, CredentialHandlerError::ActivateRotation { .. }));

    let runtime_credentials = RuntimeCredentialRegistry::default();
    let runtime_handler = CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials.clone());
    let report = recover_pending_credential_activations_from_events(
        events.events_as_raw_stream_scan(),
        &events,
        &runtime_handler,
    )
    .await;

    assert_eq!(report.planned_recoveries(), 1);
    assert_eq!(report.recovered_rotations(), 1);
    assert_eq!(report.failed_recoveries(), 0);
    assert_eq!(
        runtime_credentials
            .resolver(secrets)
            .resolve(&runtime_key(), CredentialKind::WebhookSecret)
            .await
            .unwrap()
            .as_plaintext()
            .unwrap()
            .as_str(),
        "new-secret"
    );
}

#[tokio::test]
async fn worker_continues_when_recovery_metadata_is_not_available() {
    let store = WorkerTestStreamStore::default();
    store.push_credential_event(credential_id().as_str(), write_requested_event());
    let events = store.events_as_raw_stream_scan();
    let secrets = UnavailableMetadataSecretStore;
    let runtime_credentials = RuntimeCredentialRegistry::default();
    let runtime_handler = CredentialRuntimeHandler::new(store.clone(), secrets, runtime_credentials);

    let report = recover_pending_credential_activations_from_events(events, &store, &runtime_handler).await;

    assert_eq!(report.planned_recoveries(), 1);
    assert_eq!(report.recovered_writes(), 0);
    assert_eq!(report.failed_recoveries(), 1);
}

#[tokio::test]
async fn worker_recovers_other_credentials_when_one_credential_stream_read_fails() {
    let events = WorkerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    events.fail_append_at(2);
    handler.put(put_command("primary-secret")).await.unwrap_err();
    let secondary_integration_id = SourceIntegrationId::new("secondary").unwrap();
    events.fail_append_at(4);
    handler
        .put(PutCredential::new(
            CredentialId::new("openbao:tenant-1:github/secondary:webhook_secret").unwrap(),
            CredentialScope::integration(owner_id(), SourceKind::GitHub, secondary_integration_id.clone()),
            CredentialKind::WebhookSecret,
            SecretString::new("secondary-secret").unwrap(),
        ))
        .await
        .unwrap_err();
    events.fail_read_stream(credential_id().as_str());

    let runtime_credentials = RuntimeCredentialRegistry::default();
    let runtime_handler = CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials.clone());
    let report = recover_pending_credential_activations_from_events(
        events.events_as_raw_stream_scan(),
        &events,
        &runtime_handler,
    )
    .await;

    assert_eq!(report.changed_credentials(), 2);
    assert_eq!(report.planned_recoveries(), 1);
    assert_eq!(report.recovered_writes(), 1);
    assert_eq!(report.failed_recoveries(), 1);
    assert_eq!(
        runtime_credentials
            .resolver(secrets)
            .resolve(
                &RuntimeIntegrationKey::new(SourceKind::GitHub, &secondary_integration_id),
                CredentialKind::WebhookSecret
            )
            .await
            .unwrap()
            .as_plaintext()
            .unwrap()
            .as_str(),
        "secondary-secret"
    );
}

#[tokio::test]
async fn checkpoint_store_loads_default_when_record_is_missing() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());

    let checkpoint = checkpoints.load().await.unwrap();

    assert_eq!(checkpoint.last_scanned_sequence(), 0);
    assert_eq!(kv.entry_calls(), vec![CHECKPOINT_KEY.to_string()]);
    assert!(kv.create_calls().is_empty());
    assert!(kv.update_calls().is_empty());
}

#[tokio::test]
async fn checkpoint_store_creates_checkpoint_when_record_is_missing() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());

    checkpoints.save(CredentialRecoveryCheckpoint::new(42)).await.unwrap();

    let calls = kv.create_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, CHECKPOINT_KEY);
    assert_eq!(decode_checkpoint(&calls[0].1).unwrap().last_scanned_sequence(), 42);
    assert!(kv.update_calls().is_empty());
}

#[tokio::test]
async fn checkpoint_store_updates_checkpoint_when_record_exists() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(
        Bytes::from(encode_checkpoint(CredentialRecoveryCheckpoint::new(7))),
        11,
        kv::Operation::Put,
    );
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());

    checkpoints.save(CredentialRecoveryCheckpoint::new(43)).await.unwrap();

    let calls = kv.update_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, CHECKPOINT_KEY);
    assert_eq!(decode_checkpoint(&calls[0].1).unwrap().last_scanned_sequence(), 43);
    assert_eq!(calls[0].2, 11);
    assert!(kv.create_calls().is_empty());
}

#[tokio::test]
async fn checkpoint_codec_preserves_retry_state() {
    let original = CredentialRecoveryCheckpoint::with_failure_state(9, 3, Some(100), Some(220));

    let decoded = decode_checkpoint(&encode_checkpoint(original)).unwrap();

    assert_eq!(decoded.last_scanned_sequence(), 9);
    assert_eq!(decoded.consecutive_failure_count(), 3);
    assert_eq!(decoded.first_failure_unix_seconds, Some(100));
    assert_eq!(decoded.retry_after_unix_seconds(), Some(220));
}

#[tokio::test]
async fn worker_advances_checkpoint_after_successful_recovery_scan() {
    let events = WorkerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    events.fail_append_at(2);
    handler.put(put_command("super-secret")).await.unwrap_err();

    let stream = raw_stream_with_events(events.events_as_raw_stream_scan()).await;
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    kv.enqueue_entry_none();
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());
    let runtime_credentials = RuntimeCredentialRegistry::default();
    let runtime_handler = CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials.clone());

    let report = recover_pending_credential_activations(&stream, &events, &runtime_handler, &checkpoints)
        .await
        .unwrap();

    assert_eq!(report.scanned_events(), 1);
    assert_eq!(report.planned_recoveries(), 1);
    assert_eq!(report.recovered_writes(), 1);
    assert_eq!(report.failed_recoveries(), 0);
    assert_eq!(report.checkpoint_loaded_sequence(), 0);
    assert_eq!(report.checkpoint_advanced_to(), Some(1));
    assert_eq!(report.checkpoint_failure_count(), 0);
    assert_eq!(report.checkpoint_retry_after_unix_seconds(), None);
    assert!(!report.retry_delayed());
    assert!(!report.stuck_recovery());
    assert_eq!(stream.raw_message_calls(), vec![1]);
    let calls = kv.create_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(decode_checkpoint(&calls[0].1).unwrap().last_scanned_sequence(), 1);
}

#[tokio::test]
async fn worker_starts_scan_after_loaded_checkpoint() {
    let events = WorkerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    events.fail_append_at(2);
    handler.put(put_command("super-secret")).await.unwrap_err();

    let raw_events = vec![
        stream_event("gateway.credentials.events.v1.old", 1, write_requested_event()),
        stream_event("gateway.credentials.events.v1.current", 2, write_requested_event()),
    ];
    let stream = raw_stream_with_events(raw_events).await;
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(
        Bytes::from(encode_checkpoint(CredentialRecoveryCheckpoint::new(1))),
        3,
        kv::Operation::Put,
    );
    kv.enqueue_entry(
        Bytes::from(encode_checkpoint(CredentialRecoveryCheckpoint::new(1))),
        4,
        kv::Operation::Put,
    );
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());
    let runtime_credentials = RuntimeCredentialRegistry::default();
    let runtime_handler = CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials);

    let report = recover_pending_credential_activations(&stream, &events, &runtime_handler, &checkpoints)
        .await
        .unwrap();

    assert_eq!(report.scanned_events(), 1);
    assert_eq!(report.checkpoint_loaded_sequence(), 1);
    assert_eq!(report.checkpoint_advanced_to(), Some(2));
    assert_eq!(stream.raw_message_calls(), vec![2]);
    let calls = kv.update_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(decode_checkpoint(&calls[0].1).unwrap().last_scanned_sequence(), 2);
    assert_eq!(calls[0].2, 4);
}

#[tokio::test]
async fn worker_does_not_advance_checkpoint_when_recovery_fails() {
    let store = WorkerTestStreamStore::default();
    store.push_credential_event(credential_id().as_str(), write_requested_event());
    let stream = raw_stream_with_events(store.events_as_raw_stream_scan()).await;
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());
    let secrets = UnavailableMetadataSecretStore;
    let runtime_credentials = RuntimeCredentialRegistry::default();
    let runtime_handler = CredentialRuntimeHandler::new(store.clone(), secrets, runtime_credentials);

    let report = recover_pending_credential_activations(&stream, &store, &runtime_handler, &checkpoints)
        .await
        .unwrap();

    assert_eq!(report.scanned_events(), 1);
    assert_eq!(report.planned_recoveries(), 1);
    assert_eq!(report.failed_recoveries(), 1);
    assert_eq!(report.checkpoint_loaded_sequence(), 0);
    assert_eq!(report.checkpoint_advanced_to(), None);
    assert_eq!(report.checkpoint_failure_count(), 1);
    assert!(report.checkpoint_retry_after_unix_seconds().is_some());
    assert_eq!(stream.raw_message_calls(), vec![1]);
    let calls = kv.create_calls();
    assert_eq!(calls.len(), 1);
    let saved = decode_checkpoint(&calls[0].1).unwrap();
    assert_eq!(saved.last_scanned_sequence(), 0);
    assert_eq!(saved.consecutive_failure_count(), 1);
    assert!(saved.first_failure_unix_seconds.is_some());
    assert!(saved.retry_after_unix_seconds().is_some());
    assert!(kv.update_calls().is_empty());
}

#[tokio::test]
async fn worker_records_write_failure_and_advances_checkpoint_when_pending_secret_is_missing() {
    let store = WorkerTestStreamStore::default();
    store.push_credential_event(credential_id().as_str(), write_requested_event());
    let stream = raw_stream_with_events(store.events_as_raw_stream_scan()).await;
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());
    let runtime_handler = CredentialRuntimeHandler::new(
        store.clone(),
        MockOpenBaoSecretStore::default(),
        RuntimeCredentialRegistry::default(),
    );

    let report = recover_pending_credential_activations(&stream, &store, &runtime_handler, &checkpoints)
        .await
        .unwrap();

    assert_eq!(report.failed_recoveries(), 0);
    assert_eq!(report.checkpoint_advanced_to(), Some(1));
    assert!(!report.stuck_recovery());
    let (state, _) = load_credential_state(&store, &credential_id()).await.unwrap();
    assert!(matches!(
        state.state.as_ref(),
        Some(CredentialStateSnapshotCase::WriteFailed(_))
    ));
}

#[tokio::test]
async fn worker_recovers_past_undecodable_event_and_advances_checkpoint() {
    let events = WorkerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    events.fail_append_at(2);
    handler.put(put_command("super-secret")).await.unwrap_err();

    let raw_events = vec![
        undecodable_stream_event(1),
        stream_event("gateway.credentials.events.v1.subject", 2, write_requested_event()),
    ];
    let stream = raw_stream_with_events(raw_events).await;
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());
    let runtime_credentials = RuntimeCredentialRegistry::default();
    let runtime_handler = CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials.clone());

    let report = recover_pending_credential_activations(&stream, &events, &runtime_handler, &checkpoints)
        .await
        .unwrap();

    assert_eq!(report.scanned_events(), 2);
    assert_eq!(report.decoded_events(), 1);
    assert_eq!(report.invalid_events(), 1);
    assert_eq!(report.planned_recoveries(), 1);
    assert_eq!(report.recovered_writes(), 1);
    assert_eq!(report.failed_recoveries(), 0);
    assert_eq!(report.checkpoint_advanced_to(), Some(2));
    assert_eq!(report.checkpoint_failure_count(), 0);
    assert_eq!(report.checkpoint_retry_after_unix_seconds(), None);
    assert_eq!(stream.raw_message_calls(), vec![1, 2]);
    let calls = kv.create_calls();
    assert_eq!(calls.len(), 1);
    let saved = decode_checkpoint(&calls[0].1).unwrap();
    assert_eq!(saved.last_scanned_sequence(), 2);
    assert_eq!(saved.consecutive_failure_count(), 0);
    assert_eq!(
        runtime_credentials
            .resolver(secrets)
            .resolve(&runtime_key(), CredentialKind::WebhookSecret)
            .await
            .unwrap()
            .as_plaintext()
            .unwrap()
            .as_str(),
        "super-secret"
    );
}

#[tokio::test]
async fn worker_saves_retry_backoff_when_recovery_fails() {
    let store = WorkerTestStreamStore::default();
    store.push_credential_event(credential_id().as_str(), write_requested_event());
    let stream = raw_stream_with_events(store.events_as_raw_stream_scan()).await;
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    kv.enqueue_entry_none();
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());
    let secrets = UnavailableMetadataSecretStore;
    let runtime_credentials = RuntimeCredentialRegistry::default();
    let runtime_handler = CredentialRuntimeHandler::new(store.clone(), secrets, runtime_credentials);
    let now = UNIX_EPOCH + Duration::from_secs(1_000);

    let report =
        recover_pending_credential_activations_at(&stream, &store, &runtime_handler, &checkpoints, now, test_policy())
            .await
            .unwrap();

    assert_eq!(report.failed_recoveries(), 1);
    assert_eq!(report.checkpoint_failure_count(), 1);
    assert_eq!(report.checkpoint_retry_after_unix_seconds(), Some(1_010));
    assert_eq!(stream.raw_message_calls(), vec![1]);
    let calls = kv.create_calls();
    assert_eq!(calls.len(), 1);
    let saved = decode_checkpoint(&calls[0].1).unwrap();
    assert_eq!(saved.last_scanned_sequence(), 0);
    assert_eq!(saved.consecutive_failure_count(), 1);
    assert_eq!(saved.first_failure_unix_seconds, Some(1_000));
    assert_eq!(saved.retry_after_unix_seconds(), Some(1_010));
}

#[tokio::test]
async fn worker_skips_scan_until_retry_after_is_reached() {
    let store = WorkerTestStreamStore::default();
    store.push_credential_event(credential_id().as_str(), write_requested_event());
    let stream = raw_stream_with_events(store.events_as_raw_stream_scan()).await;
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(
        Bytes::from(encode_checkpoint(CredentialRecoveryCheckpoint::with_failure_state(
            0,
            2,
            Some(900),
            Some(1_000),
        ))),
        4,
        kv::Operation::Put,
    );
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());
    let secrets = UnavailableMetadataSecretStore;
    let runtime_credentials = RuntimeCredentialRegistry::default();
    let runtime_handler = CredentialRuntimeHandler::new(store.clone(), secrets, runtime_credentials);

    let report = recover_pending_credential_activations_at(
        &stream,
        &store,
        &runtime_handler,
        &checkpoints,
        UNIX_EPOCH + Duration::from_secs(999),
        test_policy(),
    )
    .await
    .unwrap();

    assert_eq!(report.scanned_events(), 0);
    assert_eq!(report.failed_recoveries(), 0);
    assert_eq!(report.checkpoint_loaded_sequence(), 0);
    assert_eq!(report.checkpoint_failure_count(), 2);
    assert_eq!(report.checkpoint_retry_after_unix_seconds(), Some(1_000));
    assert!(report.retry_delayed());
    assert!(!report.stuck_recovery());
    assert!(stream.raw_message_calls().is_empty());
    assert!(kv.create_calls().is_empty());
    assert!(kv.update_calls().is_empty());
}

#[tokio::test]
async fn worker_reports_stuck_recovery_after_failure_age_threshold() {
    let store = WorkerTestStreamStore::default();
    let stream = raw_stream_with_events(Vec::new()).await;
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(
        Bytes::from(encode_checkpoint(CredentialRecoveryCheckpoint::with_failure_state(
            0,
            4,
            Some(100),
            Some(1_000),
        ))),
        4,
        kv::Operation::Put,
    );
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv);
    let secrets = MockOpenBaoSecretStore::default();
    let runtime_credentials = RuntimeCredentialRegistry::default();
    let runtime_handler = CredentialRuntimeHandler::new(store.clone(), secrets, runtime_credentials);

    let report = recover_pending_credential_activations_at(
        &stream,
        &store,
        &runtime_handler,
        &checkpoints,
        UNIX_EPOCH + Duration::from_secs(999),
        test_policy(),
    )
    .await
    .unwrap();

    assert!(report.retry_delayed());
    assert!(report.stuck_recovery());
    assert_eq!(report.checkpoint_failure_count(), 4);
}

#[tokio::test]
async fn worker_resets_retry_state_after_successful_recovery() {
    let events = WorkerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    events.fail_append_at(2);
    handler.put(put_command("super-secret")).await.unwrap_err();

    let stream = raw_stream_with_events(events.events_as_raw_stream_scan()).await;
    let checkpoint = CredentialRecoveryCheckpoint::with_failure_state(0, 2, Some(1_000), Some(1_010));
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(Bytes::from(encode_checkpoint(checkpoint)), 7, kv::Operation::Put);
    kv.enqueue_entry(Bytes::from(encode_checkpoint(checkpoint)), 8, kv::Operation::Put);
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());
    let runtime_credentials = RuntimeCredentialRegistry::default();
    let runtime_handler = CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials);

    let report = recover_pending_credential_activations_at(
        &stream,
        &events,
        &runtime_handler,
        &checkpoints,
        UNIX_EPOCH + Duration::from_secs(1_011),
        test_policy(),
    )
    .await
    .unwrap();

    assert_eq!(report.failed_recoveries(), 0);
    assert_eq!(report.recovered_writes(), 1);
    assert_eq!(report.checkpoint_advanced_to(), Some(1));
    assert_eq!(report.checkpoint_failure_count(), 0);
    assert_eq!(report.checkpoint_retry_after_unix_seconds(), None);
    let calls = kv.update_calls();
    assert_eq!(calls.len(), 1);
    let saved = decode_checkpoint(&calls[0].1).unwrap();
    assert_eq!(saved.last_scanned_sequence(), 1);
    assert_eq!(saved.consecutive_failure_count(), 0);
    assert_eq!(saved.first_failure_unix_seconds, None);
    assert_eq!(saved.retry_after_unix_seconds(), None);
}

#[test]
fn recovery_report_metric_outcome_is_bounded() {
    assert_eq!(
        CredentialRecoveryReport {
            retry_delayed: true,
            ..Default::default()
        }
        .metric_outcome(),
        "retry_delayed"
    );
    assert_eq!(
        CredentialRecoveryReport {
            invalid_events: 1,
            failed_recoveries: 1,
            ..Default::default()
        }
        .metric_outcome(),
        "invalid_event"
    );
    assert_eq!(
        CredentialRecoveryReport {
            failed_recoveries: 1,
            ..Default::default()
        }
        .metric_outcome(),
        "failed_recovery"
    );
    assert_eq!(
        CredentialRecoveryReport {
            checkpoint_advanced_to: Some(7),
            ..Default::default()
        }
        .metric_outcome(),
        "advanced"
    );
    assert_eq!(
        CredentialRecoveryReport {
            recovered_writes: 1,
            ..Default::default()
        }
        .metric_outcome(),
        "recovered"
    );
    assert_eq!(
        CredentialRecoveryReport {
            stuck_recovery: true,
            ..Default::default()
        }
        .metric_outcome(),
        "stuck"
    );
    assert_eq!(
        CredentialRecoveryReport {
            retry_delayed: true,
            stuck_recovery: true,
            ..Default::default()
        }
        .metric_outcome(),
        "stuck"
    );
    assert_eq!(CredentialRecoveryReport::default().metric_outcome(), "idle");
}

#[test]
fn recovery_metrics_recording_is_infallible() {
    let metrics = CredentialRecoveryMetrics::new();
    let report = CredentialRecoveryReport {
        scanned_events: 5,
        planned_recoveries: 2,
        recovered_writes: 1,
        recovered_rotations: 1,
        failed_recoveries: 1,
        stuck_recovery: true,
        ..Default::default()
    };

    metrics.record_report(&report);
    metrics.record_error("test_error");
}

#[tokio::test]
async fn provision_checkpoint_bucket_uses_expected_runtime_contract() {
    let client = MockJetStreamKvClient::new();

    let _store: MockJetStreamKvStore = provision_checkpoint_bucket(&client).await.unwrap();

    let configs = client.create_configs();
    assert_eq!(configs.len(), 1);
    assert_eq!(configs[0].bucket, CREDENTIAL_WORKER_CHECKPOINT_BUCKET);
    assert_eq!(configs[0].history, 1);
    assert_eq!(configs[0].max_age, Duration::ZERO);
}

#[tokio::test]
async fn provision_checkpoint_bucket_opens_existing_bucket() {
    let existing = MockJetStreamKvStore::new();
    let client = MockJetStreamKvClient::new();
    client.fail_create_already_exists();
    client.set_get_result(existing);

    let _store: MockJetStreamKvStore = provision_checkpoint_bucket(&client).await.unwrap();

    assert_eq!(
        client.requested_buckets(),
        vec![CREDENTIAL_WORKER_CHECKPOINT_BUCKET.to_string()]
    );
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

fn stream_event(stream_id: &str, stream_position: u64, event: v1::CredentialEvent) -> StreamEvent {
    StreamEvent {
        stream_id: stream_id.to_string(),
        event: runtime_event(stream_position, event),
        stream_position: position(stream_position),
        recorded_at: Utc::now(),
    }
}

fn undecodable_stream_event(stream_position: u64) -> StreamEvent {
    let mut event = stream_event(
        "gateway.credentials.events.v1.subject",
        stream_position,
        write_requested_event(),
    );
    event.event.content = b"\0".to_vec();
    event
}

fn write_requested_event() -> v1::CredentialEvent {
    v1::CredentialEvent {
        event: Some(write_requested_to_proto(&credential_id(), &scope(), CredentialKind::WebhookSecret).into()),
    }
}

async fn raw_stream_with_events(events: Vec<StreamEvent>) -> trogon_nats::jetstream::MockJetStreamStream {
    let stream = MockJetStreamConsumerFactory::new();
    stream.set_info(make_stream_info(
        events
            .iter()
            .map(|event| event.stream_position.as_u64())
            .max()
            .unwrap_or(0),
    ));
    for event in events {
        stream.add_raw_message(event.stream_position.as_u64(), raw_stream_message(event));
    }
    stream.get_stream("gateway.credentials.events.v1").await.unwrap()
}

fn raw_stream_message(event: StreamEvent) -> StreamMessage {
    let mut headers = HeaderMap::new();
    headers.insert(NATS_MESSAGE_ID, event.event.id.to_string().as_str());
    headers.insert(TROGON_EVENT_TYPE, event.event.r#type.as_str());
    StreamMessage {
        subject: event.stream_id.into(),
        sequence: event.stream_position.as_u64(),
        headers,
        payload: Bytes::from(event.event.content),
        time: OffsetDateTime::UNIX_EPOCH,
    }
}

fn make_stream_info(last_sequence: u64) -> async_nats::jetstream::stream::Info {
    serde_json::from_value(serde_json::json!({
        "config": {
            "name": "GATEWAY_CREDENTIAL_EVENTS",
            "subjects": [],
            "retention": "limits",
            "max_consumers": -1,
            "max_msgs": -1,
            "max_bytes": -1,
            "discard": "old",
            "max_age": 0,
            "storage": "file",
            "num_replicas": 1
        },
        "created": "1970-01-01T00:00:00Z",
        "state": {
            "messages": last_sequence,
            "bytes": 0_u64,
            "first_seq": if last_sequence == 0 { 0_u64 } else { 1_u64 },
            "first_ts": "1970-01-01T00:00:00Z",
            "last_seq": last_sequence,
            "last_ts": "1970-01-01T00:00:00Z",
            "consumer_count": 0_usize,
            "num_subjects": 0_u64
        },
        "cluster": null,
        "mirror": null,
        "sources": []
    }))
    .expect("test stream info must be valid")
}

fn runtime_event(id: u64, event: v1::CredentialEvent) -> Event {
    Event {
        id: EventId::new(Uuid::from_u128(id as u128)),
        r#type: EventType::event_type(&event).unwrap().to_string(),
        content: EventEncode::encode(&event).unwrap(),
        headers: Headers::empty(),
    }
}

fn owner_id() -> CredentialOwnerId {
    CredentialOwnerId::new("tenant-1").unwrap()
}

fn integration_id() -> SourceIntegrationId {
    SourceIntegrationId::new("primary").unwrap()
}

fn scope() -> CredentialScope {
    CredentialScope::integration(owner_id(), SourceKind::GitHub, integration_id())
}

fn credential_id() -> CredentialId {
    CredentialId::new("openbao:tenant-1:github/primary:webhook_secret").unwrap()
}

fn credential_ref(version: u64) -> CredentialRef {
    CredentialRef::new(
        credential_id(),
        CredentialVersion::new(version).unwrap(),
        &scope(),
        CredentialKind::WebhookSecret,
    )
}

fn runtime_key() -> RuntimeIntegrationKey {
    RuntimeIntegrationKey::new(SourceKind::GitHub, &integration_id())
}

fn test_policy() -> CredentialRecoveryPolicy {
    CredentialRecoveryPolicy {
        initial_failure_backoff: Duration::from_secs(10),
        max_failure_backoff: Duration::from_secs(40),
        stuck_after: Duration::from_secs(100),
    }
}

fn put_command(value: &str) -> PutCredential {
    PutCredential::new(
        credential_id(),
        scope(),
        CredentialKind::WebhookSecret,
        SecretString::new(value).unwrap(),
    )
}

fn foreign_stream_event(stream_id: &str, stream_position: u64) -> StreamEvent {
    let mut event = stream_event(stream_id, stream_position, write_requested_event());
    event.event.r#type = "example.foreign.event".to_string();
    event
}

fn other_credential_id() -> CredentialId {
    CredentialId::new("openbao:tenant-1:github/secondary:webhook_secret").unwrap()
}

fn failure_reason() -> CredentialFailureReason {
    CredentialFailureReason::new("backend refused").unwrap()
}

fn active_metadata(version: u64) -> CredentialMetadata {
    CredentialMetadata::new(
        credential_ref(version),
        CredentialStatus::Active,
        StorageBackend::OpenBao,
        CredentialFingerprint::new("fingerprint").unwrap(),
    )
}

fn terminate_current_process() {
    let status = std::process::Command::new("kill")
        .args(["-TERM", &std::process::id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

fn message_of(events: &CapturedEvents, message: &str) -> Vec<trogon_std::log_capture::CapturedEvent> {
    events
        .events()
        .into_iter()
        .filter(|event| event.message() == Some(message))
        .collect()
}

async fn active_credential_ref(
    handler: &CredentialHandler<WorkerTestStreamStore, MockOpenBaoSecretStore>,
) -> CredentialRef {
    let state = handler.put(put_command("old-secret")).await.unwrap().into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    decode_active_state("active", active).unwrap().0.reference().clone()
}

#[test]
fn recovery_report_flags_every_condition_that_deserves_a_log_line() {
    assert!(!CredentialRecoveryReport::default().has_recovery_activity());
    for report in [
        CredentialRecoveryReport {
            planned_recoveries: 1,
            ..Default::default()
        },
        CredentialRecoveryReport {
            failed_recoveries: 1,
            ..Default::default()
        },
        CredentialRecoveryReport {
            invalid_events: 1,
            ..Default::default()
        },
        CredentialRecoveryReport {
            retry_delayed: true,
            ..Default::default()
        },
        CredentialRecoveryReport {
            stuck_recovery: true,
            ..Default::default()
        },
    ] {
        assert!(report.has_recovery_activity());
    }
}

#[tokio::test]
async fn recovery_skips_foreign_events_and_credentials_with_nothing_to_recover() {
    let events = WorkerTestStreamStore::default();
    let handler = CredentialHandler::new(events.clone(), MockOpenBaoSecretStore::default());
    active_credential_ref(&handler).await;
    let mut raw_events = events.events_as_raw_stream_scan();
    raw_events.push(foreign_stream_event("gateway.credentials.events.v1.subject", 3));
    events.events.lock().unwrap().push(StreamEvent {
        stream_id: credential_id().as_str().to_string(),
        event: foreign_stream_event("ignored", 3).event,
        stream_position: position(3),
        recorded_at: Utc::now(),
    });

    let plan = recovery_plan_from_credential_events(raw_events, &events).await;

    assert_eq!(plan.report.scanned_events(), 3);
    assert_eq!(plan.report.decoded_events(), 2);
    assert_eq!(plan.report.skipped_events(), 1);
    assert_eq!(plan.report.changed_credentials(), 1);
    assert_eq!(plan.report.skipped_credentials(), 1);
    assert_eq!(plan.report.planned_recoveries(), 0);
    assert!(plan.commands.is_empty());
}

#[tokio::test]
async fn recovery_logs_the_sequence_of_an_invalid_event() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let store = WorkerTestStreamStore::default();

    let plan = recovery_plan_from_credential_events(vec![undecodable_stream_event(7)], &store).await;

    assert_eq!(plan.report.invalid_events(), 1);
    let logged = message_of(&captured, "credential recovery found an invalid credential event");
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].field("stream_sequence"), Some("7"));
    assert!(logged[0].field("error").is_some());
}

#[tokio::test]
async fn recovery_reports_a_pending_write_that_belongs_to_another_credential() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let store = WorkerTestStreamStore::default();
    let foreign_write = v1::CredentialEvent {
        event: Some(write_requested_to_proto(&other_credential_id(), &scope(), CredentialKind::WebhookSecret).into()),
    };
    store.push_credential_event(credential_id().as_str(), foreign_write);
    let raw_events = vec![stream_event(
        "gateway.credentials.events.v1.subject",
        1,
        write_requested_event(),
    )];

    let plan = recovery_plan_from_credential_events(raw_events, &store).await;

    assert_eq!(plan.report.planned_recoveries(), 0);
    assert_eq!(plan.report.failed_recoveries(), 1);
    assert!(plan.commands.is_empty());
    let logged = message_of(&captured, "credential recovery planning failed");
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].field("credential_id"), Some(credential_id().as_str()));
}

#[tokio::test]
async fn recovery_logs_the_credential_whose_stream_could_not_be_replayed() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let store = WorkerTestStreamStore::default();
    store.push_credential_event(credential_id().as_str(), write_requested_event());
    store.fail_read_stream(credential_id().as_str());

    let plan = recovery_plan_from_credential_events(store.events_as_raw_stream_scan(), &store).await;

    assert_eq!(plan.report.failed_recoveries(), 1);
    let logged = message_of(&captured, "credential recovery replay failed");
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].field("credential_id"), Some(credential_id().as_str()));
}

#[tokio::test]
async fn worker_logs_a_failed_write_activation_recovery_with_its_credential() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let store = WorkerTestStreamStore::default();
    store.push_credential_event(credential_id().as_str(), write_requested_event());
    let runtime_handler = CredentialRuntimeHandler::new(
        store.clone(),
        UnavailableMetadataSecretStore,
        RuntimeCredentialRegistry::default(),
    );

    let report =
        recover_pending_credential_activations_from_events(store.events_as_raw_stream_scan(), &store, &runtime_handler)
            .await;

    assert_eq!(report.failed_recoveries(), 1);
    let logged = message_of(&captured, "credential write activation recovery failed");
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].field("credential_id"), Some(credential_id().as_str()));
    assert!(logged[0].field("error").is_some());
}

#[tokio::test]
async fn worker_logs_a_failed_rotation_activation_recovery_with_its_credential() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let events = WorkerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets);
    let active_ref = active_credential_ref(&handler).await;
    events.fail_append_at(4);
    handler
        .rotate(RotateCredential::new(
            active_ref,
            SecretString::new("new-secret").unwrap(),
        ))
        .await
        .unwrap_err();
    let runtime_handler = CredentialRuntimeHandler::new(
        events.clone(),
        UnavailableMetadataSecretStore,
        RuntimeCredentialRegistry::default(),
    );

    let report = recover_pending_credential_activations_from_events(
        events.events_as_raw_stream_scan(),
        &events,
        &runtime_handler,
    )
    .await;

    assert_eq!(report.planned_recoveries(), 1);
    assert_eq!(report.recovered_rotations(), 0);
    assert_eq!(report.failed_recoveries(), 1);
    let logged = message_of(&captured, "credential rotation activation recovery failed");
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].field("credential_id"), Some(credential_id().as_str()));
}

#[tokio::test]
async fn worker_recovers_the_version_after_a_destroyed_credential() {
    let events = WorkerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    let first = active_credential_ref(&handler).await;
    handler
        .revoke(RevokeStoredCredential::new(first.clone()))
        .await
        .unwrap();
    handler
        .destroy(DestroyStoredCredential::new(
            first,
            SecretDestroyReason::new("credential lifecycle cleanup").unwrap(),
        ))
        .await
        .unwrap();
    events.fail_append_at(7);
    handler.put(put_command("second-secret")).await.unwrap_err();
    let runtime_credentials = RuntimeCredentialRegistry::default();
    let runtime_handler = CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials.clone());

    let report = recover_pending_credential_activations_from_events(
        events.events_as_raw_stream_scan(),
        &events,
        &runtime_handler,
    )
    .await;

    assert_eq!(report.planned_recoveries(), 1);
    assert_eq!(report.recovered_writes(), 1);
    assert_eq!(report.failed_recoveries(), 0);
    assert_eq!(
        runtime_credentials
            .resolver(secrets)
            .resolve(&runtime_key(), CredentialKind::WebhookSecret)
            .await
            .unwrap()
            .as_plaintext()
            .unwrap()
            .as_str(),
        "second-secret"
    );
}

#[test]
fn every_credential_event_resolves_to_the_credential_it_belongs_to() {
    let metadata = active_metadata(1);
    let next_metadata = active_metadata(2);
    let reason = failure_reason();
    let destroy_reason = CredentialDestroyReason::new("credential lifecycle cleanup").unwrap();
    let events: Vec<CredentialEventCase> = vec![
        write_requested_to_proto(&credential_id(), &scope(), CredentialKind::WebhookSecret).into(),
        write_failed_to_proto(&credential_id(), &reason).into(),
        activated_to_proto(&metadata).into(),
        rotation_requested_to_proto(&credential_ref(1)).into(),
        rotation_failed_to_proto(&credential_ref(1), &reason).into(),
        rotated_to_proto(&credential_ref(1), &next_metadata).into(),
        revoked_to_proto(&credential_ref(2)).into(),
        destroy_requested_to_proto(&credential_ref(2), &destroy_reason).into(),
        destroy_failed_to_proto(&credential_ref(2), &reason).into(),
        destroyed_to_proto(&credential_ref(2)).into(),
    ];

    for event in &events {
        assert_eq!(event_credential_id(event).unwrap(), credential_id());
    }
}

#[tokio::test]
async fn worker_leaves_the_checkpoint_alone_when_nothing_new_was_scanned() {
    let store = WorkerTestStreamStore::default();
    let stream = raw_stream_with_events(Vec::new()).await;
    let kv = MockJetStreamKvStore::new();
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());
    let runtime_handler = CredentialRuntimeHandler::new(
        store.clone(),
        MockOpenBaoSecretStore::default(),
        RuntimeCredentialRegistry::default(),
    );

    let report = recover_pending_credential_activations(&stream, &store, &runtime_handler, &checkpoints)
        .await
        .unwrap();

    assert_eq!(report.scanned_events(), 0);
    assert_eq!(report.checkpoint_advanced_to(), None);
    assert_eq!(report.metric_outcome(), "idle");
    assert!(kv.create_calls().is_empty());
    assert!(kv.update_calls().is_empty());
}

#[tokio::test]
async fn worker_logs_why_the_checkpoint_was_not_advanced() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let store = WorkerTestStreamStore::default();
    store.push_credential_event(credential_id().as_str(), write_requested_event());
    let stream = raw_stream_with_events(store.events_as_raw_stream_scan()).await;
    let kv = MockJetStreamKvStore::new();
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv);
    let runtime_handler = CredentialRuntimeHandler::new(
        store.clone(),
        UnavailableMetadataSecretStore,
        RuntimeCredentialRegistry::default(),
    );

    let report = recover_pending_credential_activations_at(
        &stream,
        &store,
        &runtime_handler,
        &checkpoints,
        UNIX_EPOCH + Duration::from_secs(1_000),
        test_policy(),
    )
    .await
    .unwrap();

    assert_eq!(report.failed_recoveries(), 1);
    let logged = message_of(&captured, "credential recovery checkpoint was not advanced");
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].field("failed_recoveries"), Some("1"));
    assert_eq!(logged[0].field("checkpoint_failure_count"), Some("1"));
    assert_eq!(
        logged[0].field("checkpoint_retry_after_unix_seconds"),
        Some("Some(1010)")
    );
    assert_eq!(logged[0].field("stuck_recovery"), Some("false"));
}

#[tokio::test]
async fn worker_logs_when_recovery_is_stuck_behind_retry_backoff() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let store = WorkerTestStreamStore::default();
    let stream = raw_stream_with_events(Vec::new()).await;
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(
        Bytes::from(encode_checkpoint(CredentialRecoveryCheckpoint::with_failure_state(
            3,
            4,
            Some(100),
            Some(1_000),
        ))),
        4,
        kv::Operation::Put,
    );
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv);
    let runtime_handler = CredentialRuntimeHandler::new(
        store.clone(),
        MockOpenBaoSecretStore::default(),
        RuntimeCredentialRegistry::default(),
    );

    let report = recover_pending_credential_activations_at(
        &stream,
        &store,
        &runtime_handler,
        &checkpoints,
        UNIX_EPOCH + Duration::from_secs(999),
        test_policy(),
    )
    .await
    .unwrap();

    assert!(report.stuck_recovery());
    let logged = message_of(&captured, "credential recovery is stuck behind retry backoff");
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].field("checkpoint_loaded_sequence"), Some("3"));
    assert_eq!(logged[0].field("checkpoint_failure_count"), Some("4"));
    assert_eq!(
        logged[0].field("checkpoint_retry_after_unix_seconds"),
        Some("Some(1000)")
    );
}

#[tokio::test]
async fn recovery_pass_surfaces_a_checkpoint_load_failure() {
    let store = WorkerTestStreamStore::default();
    let stream = raw_stream_with_events(Vec::new()).await;
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_error(kv::EntryErrorKind::TimedOut);
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv);
    let runtime_handler = CredentialRuntimeHandler::new(
        store.clone(),
        MockOpenBaoSecretStore::default(),
        RuntimeCredentialRegistry::default(),
    );

    let error = recover_pending_credential_activations(&stream, &store, &runtime_handler, &checkpoints)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        CredentialRecoveryWorkerError::Checkpoint {
            source: CredentialRecoveryCheckpointStoreError::Backend { .. }
        }
    ));
}

#[tokio::test]
async fn run_keeps_recovering_until_a_termination_signal_arrives() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let store = WorkerTestStreamStore::default();
    store.push_credential_event(credential_id().as_str(), write_requested_event());
    let stream = raw_stream_with_events(store.events_as_raw_stream_scan()).await;
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_error(kv::EntryErrorKind::TimedOut);
    kv.enqueue_entry_none();
    kv.enqueue_entry_none();
    kv.enqueue_entry(
        Bytes::from(encode_checkpoint(CredentialRecoveryCheckpoint::new(1))),
        3,
        kv::Operation::Put,
    );
    let runtime_handler = CredentialRuntimeHandler::new(
        store.clone(),
        UnavailableMetadataSecretStore,
        RuntimeCredentialRegistry::default(),
    );
    let worker = run(
        stream,
        store.clone(),
        runtime_handler,
        CredentialRecoveryKvCheckpointStore::new(kv.clone()),
        Duration::from_millis(5),
    );
    let stop = async {
        while kv.entry_calls().len() < 5 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        loop {
            terminate_current_process();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };

    tokio::time::timeout(Duration::from_secs(30), async {
        tokio::select! {
            () = worker => {}
            () = stop => {}
        }
    })
    .await
    .unwrap();

    assert_eq!(message_of(&captured, "credential recovery pass failed").len(), 1);
    assert!(!message_of(&captured, "credential recovery pass completed").is_empty());
    assert_eq!(message_of(&captured, "credential recovery worker stopped").len(), 1);
    let completed = message_of(&captured, "credential recovery pass completed");
    assert_eq!(completed[0].field("failed_recoveries"), Some("1"));
    assert_eq!(completed[0].field("planned_recoveries"), Some("1"));
}

#[tokio::test]
async fn checkpoint_store_treats_a_deleted_or_purged_record_as_missing() {
    for operation in [kv::Operation::Delete, kv::Operation::Purge] {
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry(
            Bytes::from(encode_checkpoint(CredentialRecoveryCheckpoint::new(9))),
            3,
            operation,
        );
        let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv);

        assert_eq!(
            checkpoints.load().await.unwrap(),
            CredentialRecoveryCheckpoint::default()
        );
    }
}

#[tokio::test]
async fn checkpoint_store_reports_a_backend_failure_while_loading() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_error(kv::EntryErrorKind::TimedOut);
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv);

    let error = checkpoints.load().await.unwrap_err();

    assert!(matches!(error, CredentialRecoveryCheckpointStoreError::Backend { .. }));
    assert!(
        error
            .to_string()
            .starts_with("credential recovery checkpoint backend failed")
    );
}

#[tokio::test]
async fn checkpoint_store_reports_a_codec_failure_for_a_corrupt_record() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(Bytes::from_static(b"\xff\xff\xff\xff"), 3, kv::Operation::Put);
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv);

    let error = checkpoints.load().await.unwrap_err();

    assert!(matches!(error, CredentialRecoveryCheckpointStoreError::Codec { .. }));
}

#[tokio::test]
async fn checkpoint_store_retries_creation_after_losing_the_creation_race() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    kv.enqueue_entry_none();
    kv.enqueue_create_result(Err(kv::CreateErrorKind::AlreadyExists));
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());

    checkpoints.save(CredentialRecoveryCheckpoint::new(5)).await.unwrap();

    assert_eq!(kv.create_calls().len(), 2);
    assert_eq!(kv.entry_calls().len(), 2);
}

#[tokio::test]
async fn checkpoint_store_reports_a_backend_failure_while_creating() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    kv.enqueue_create_result(Err(kv::CreateErrorKind::Other));
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());

    let error = checkpoints
        .save(CredentialRecoveryCheckpoint::new(5))
        .await
        .unwrap_err();

    assert!(matches!(error, CredentialRecoveryCheckpointStoreError::Backend { .. }));
    assert_eq!(kv.create_calls().len(), 1);
}

#[tokio::test]
async fn checkpoint_store_recreates_a_deleted_record() {
    for operation in [kv::Operation::Delete, kv::Operation::Purge] {
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry(Bytes::new(), 3, operation);
        let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());

        checkpoints.save(CredentialRecoveryCheckpoint::new(6)).await.unwrap();

        let calls = kv.create_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(decode_checkpoint(&calls[0].1).unwrap().last_scanned_sequence(), 6);
        assert!(kv.update_calls().is_empty());
    }
}

#[tokio::test]
async fn checkpoint_store_retries_recreating_a_deleted_record_after_losing_the_race() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(Bytes::new(), 3, kv::Operation::Delete);
    kv.enqueue_entry(Bytes::new(), 4, kv::Operation::Delete);
    kv.enqueue_create_result(Err(kv::CreateErrorKind::AlreadyExists));
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());

    checkpoints.save(CredentialRecoveryCheckpoint::new(6)).await.unwrap();

    assert_eq!(kv.create_calls().len(), 2);
}

#[tokio::test]
async fn checkpoint_store_reports_a_backend_failure_while_recreating_a_deleted_record() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(Bytes::new(), 3, kv::Operation::Delete);
    kv.enqueue_create_result(Err(kv::CreateErrorKind::Other));
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv);

    let error = checkpoints
        .save(CredentialRecoveryCheckpoint::new(6))
        .await
        .unwrap_err();

    assert!(matches!(error, CredentialRecoveryCheckpointStoreError::Backend { .. }));
}

#[tokio::test]
async fn checkpoint_store_retries_the_update_after_a_revision_conflict() {
    let kv = MockJetStreamKvStore::new();
    let existing = Bytes::from(encode_checkpoint(CredentialRecoveryCheckpoint::new(1)));
    kv.enqueue_entry(existing.clone(), 3, kv::Operation::Put);
    kv.enqueue_entry(existing, 4, kv::Operation::Put);
    kv.enqueue_update_result(Err(kv::UpdateErrorKind::WrongLastRevision));
    kv.enqueue_update_result(Ok(5));
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());

    checkpoints.save(CredentialRecoveryCheckpoint::new(8)).await.unwrap();

    let revisions: Vec<u64> = kv.update_calls().iter().map(|call| call.2).collect();
    assert_eq!(revisions, [3, 4]);
}

#[tokio::test]
async fn checkpoint_store_reports_a_backend_failure_while_updating() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(
        Bytes::from(encode_checkpoint(CredentialRecoveryCheckpoint::new(1))),
        3,
        kv::Operation::Put,
    );
    kv.enqueue_update_result(Err(kv::UpdateErrorKind::TimedOut));
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv);

    let error = checkpoints
        .save(CredentialRecoveryCheckpoint::new(8))
        .await
        .unwrap_err();

    assert!(matches!(error, CredentialRecoveryCheckpointStoreError::Backend { .. }));
}

#[tokio::test]
async fn checkpoint_store_gives_up_after_repeated_revision_conflicts() {
    let kv = MockJetStreamKvStore::new();
    let existing = Bytes::from(encode_checkpoint(CredentialRecoveryCheckpoint::new(1)));
    for revision in 3..6 {
        kv.enqueue_entry(existing.clone(), revision, kv::Operation::Put);
        kv.enqueue_update_result(Err(kv::UpdateErrorKind::WrongLastRevision));
    }
    let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());

    let error = checkpoints
        .save(CredentialRecoveryCheckpoint::new(8))
        .await
        .unwrap_err();

    assert!(matches!(error, CredentialRecoveryCheckpointStoreError::Conflict));
    assert_eq!(kv.update_calls().len(), 3);
}

#[tokio::test]
async fn open_checkpoint_store_hands_back_a_store_that_persists_checkpoints() {
    let client = MockJetStreamKvClient::new();
    let kv = MockJetStreamKvStore::new();
    client.set_create_result(kv.clone());

    let checkpoints = open_checkpoint_store(&client).await.unwrap();
    checkpoints.save(CredentialRecoveryCheckpoint::new(11)).await.unwrap();

    let calls = kv.create_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(decode_checkpoint(&calls[0].1).unwrap().last_scanned_sequence(), 11);
}

#[tokio::test]
async fn open_checkpoint_store_surfaces_bucket_creation_failure() {
    let client = MockJetStreamKvClient::new();
    client.fail_create(context::CreateKeyValueErrorKind::TimedOut);

    let error = open_checkpoint_store::<_, MockJetStreamKvStore>(&client)
        .await
        .unwrap_err();

    assert!(matches!(error, CredentialRecoveryCheckpointOpenError::Create(_)));
}

#[tokio::test]
async fn open_checkpoint_store_surfaces_failure_to_open_the_existing_bucket() {
    let client = MockJetStreamKvClient::new();
    client.fail_create_already_exists();
    client.fail_get(context::KeyValueErrorKind::GetBucket);

    let error = open_checkpoint_store::<_, MockJetStreamKvStore>(&client)
        .await
        .unwrap_err();

    assert!(matches!(error, CredentialRecoveryCheckpointOpenError::OpenExisting(_)));
}

#[tokio::test]
async fn open_checkpoint_store_surfaces_failure_to_inspect_the_bucket() {
    let client = MockJetStreamKvClient::new();
    let kv = MockJetStreamKvStore::new();
    kv.fail_status(kv::StatusErrorKind::TimedOut);
    client.set_create_result(kv);

    let error = open_checkpoint_store(&client).await.err().unwrap();

    assert!(matches!(error, CredentialRecoveryCheckpointOpenError::Inspect(_)));
}

#[tokio::test]
async fn open_checkpoint_store_rejects_a_bucket_with_the_wrong_retention() {
    let client = MockJetStreamKvClient::new();
    let kv = MockJetStreamKvStore::new();
    kv.set_settings(5, Duration::from_secs(60), true, None);
    client.set_create_result(kv);

    let error = open_checkpoint_store(&client).await.err().unwrap();

    let CredentialRecoveryCheckpointOpenError::Incompatible { source } = &error else {
        panic!("expected an incompatible bucket, got {error}");
    };
    assert_eq!(source.actual_history, 5);
    assert_eq!(source.actual_max_age, Duration::from_secs(60));
    assert_eq!(source.expected_history, 1);
    assert_eq!(source.expected_max_age, Duration::ZERO);
}
