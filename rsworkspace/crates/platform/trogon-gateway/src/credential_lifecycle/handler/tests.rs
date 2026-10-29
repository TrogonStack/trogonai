use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use testcontainers_modules::testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, Event, EventEncode, EventId, EventType, Headers, ReadSnapshotRequest,
    ReadSnapshotResponse, ReadStreamResponse, Snapshot, SnapshotRead, SnapshotWrite, StreamEvent, StreamPosition,
    StreamWritePrecondition, WriteSnapshotRequest, WriteSnapshotResponse,
};

use super::*;
use crate::credential::commands::domain::{
    CredentialFingerprint, CredentialMetadata, CredentialOwnerId, CredentialStatus, StorageBackend,
};
use crate::credential::commands::proto::{
    activated_to_proto, decode_active_state, pending_write_to_proto_state, rotation_requested_to_proto,
    write_requested_to_proto,
};
use crate::credential_runtime_projection::{
    RuntimeCredentialError, RuntimeCredentialRegistry, RuntimeIntegrationKey, RuntimeIntegrationProjection,
};
use crate::secret_store::openbao_secret_store::OpenBaoSecretStore;
use crate::secret_store::{MockOpenBaoSecretStore, SecretStoreError, SecretStoreGet};
use crate::source_integration_id::SourceIntegrationId;
use trogonai_proto::gateway::credentials::CredentialEventCase;

const OPENBAO_IMAGE: &str = "openbao/openbao";
const OPENBAO_IMAGE_TAG: &str = "2.5.5";
const OPENBAO_DEV_ROOT_TOKEN: &str = "dev-only-token";
const OPENBAO_PORT: u16 = 8200;

#[derive(Debug, thiserror::Error)]
#[error("handler test stream store rejected the append")]
struct HandlerTestStreamStoreError;

#[derive(Clone, Default)]
struct HandlerTestStreamStore {
    events: Arc<Mutex<Vec<StreamEvent>>>,
    snapshots: Arc<Mutex<BTreeMap<String, Snapshot<state_v1::CredentialStateSnapshot>>>>,
    write_preconditions: Arc<Mutex<Vec<StreamWritePrecondition>>>,
    fail_append_at: Arc<Mutex<Option<usize>>>,
}

impl HandlerTestStreamStore {
    fn fail_append_at(&self, append_count: usize) {
        *self.fail_append_at.lock().unwrap() = Some(append_count);
    }

    fn events(&self) -> Vec<StreamEvent> {
        self.events.lock().unwrap().clone()
    }

    fn decoded_events(&self) -> Vec<v1::CredentialEvent> {
        self.events()
            .into_iter()
            .map(|event| event.decode::<v1::CredentialEvent>().unwrap().into_decoded().unwrap())
            .collect()
    }

    fn write_preconditions(&self) -> Vec<StreamWritePrecondition> {
        self.write_preconditions.lock().unwrap().clone()
    }
}

impl StreamRead<str> for HandlerTestStreamStore {
    type Error = HandlerTestStreamStoreError;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
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

impl StreamAppend<str> for HandlerTestStreamStore {
    type Error = HandlerTestStreamStoreError;

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
                return Err(HandlerTestStreamStoreError);
            }
        }

        let current_position = current_position(&events, request.stream_id);
        match request.stream_write_precondition {
            StreamWritePrecondition::Any => {}
            StreamWritePrecondition::StreamExists if current_position.is_some() => {}
            StreamWritePrecondition::NoStream if current_position.is_none() => {}
            StreamWritePrecondition::At(position) if current_position == Some(position) => {}
            _ => return Err(HandlerTestStreamStoreError),
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

impl SnapshotRead<state_v1::CredentialStateSnapshot, str> for HandlerTestStreamStore {
    type Error = HandlerTestStreamStoreError;

    async fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, str>,
    ) -> Result<ReadSnapshotResponse<state_v1::CredentialStateSnapshot>, Self::Error> {
        Ok(ReadSnapshotResponse {
            snapshot: self.snapshots.lock().unwrap().get(request.snapshot_id).cloned(),
        })
    }
}

impl SnapshotWrite<state_v1::CredentialStateSnapshot, str> for HandlerTestStreamStore {
    type Error = HandlerTestStreamStoreError;

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
struct FailingSecretStore;

impl SecretStorePut for FailingSecretStore {
    type Error = SecretStoreError;

    async fn put(
        &self,
        scope: CredentialScope,
        kind: CredentialKind,
        _value: SecretString,
    ) -> Result<CredentialRef, Self::Error> {
        let credential = CredentialRef::new(credential_id(), CredentialVersion::initial(), &scope, kind);
        Err(SecretStoreError::BackendUnavailable {
            backend: StorageBackend::OpenBao,
            message: format!("OpenBao refused {}", credential.id()),
        })
    }
}

impl SecretStoreMetadata for FailingSecretStore {
    type Error = SecretStoreError;

    async fn metadata(&self, credential: &CredentialRef) -> Result<CredentialMetadata, Self::Error> {
        Err(SecretStoreError::Missing {
            credential: credential.clone(),
        })
    }
}

#[derive(Clone)]
struct FailingRotateSecretStore;

impl SecretStoreRotate for FailingRotateSecretStore {
    type Error = SecretStoreError;

    async fn rotate(&self, credential: &CredentialRef, _value: SecretString) -> Result<CredentialRef, Self::Error> {
        Err(SecretStoreError::BackendUnavailable {
            backend: StorageBackend::OpenBao,
            message: format!("OpenBao refused rotation for {}", credential.id()),
        })
    }
}

impl SecretStoreMetadata for FailingRotateSecretStore {
    type Error = SecretStoreError;

    async fn metadata(&self, credential: &CredentialRef) -> Result<CredentialMetadata, Self::Error> {
        Err(SecretStoreError::Missing {
            credential: credential.clone(),
        })
    }
}

#[derive(Clone)]
struct FailingDestroySecretStore;

impl SecretStoreDestroy for FailingDestroySecretStore {
    type Error = SecretStoreError;

    async fn destroy(&self, credential: &CredentialRef, _reason: &SecretDestroyReason) -> Result<(), Self::Error> {
        Err(SecretStoreError::BackendUnavailable {
            backend: StorageBackend::OpenBao,
            message: format!("OpenBao refused destroy for {}", credential.id()),
        })
    }
}

#[derive(Clone, Default)]
struct RecordingRevokeSecretStore {
    revoked: Arc<Mutex<Vec<CredentialRef>>>,
}

impl RecordingRevokeSecretStore {
    fn revoked(&self) -> Vec<CredentialRef> {
        self.revoked.lock().unwrap().clone()
    }
}

impl SecretStoreRevoke for RecordingRevokeSecretStore {
    type Error = SecretStoreError;

    async fn revoke(&self, credential: &CredentialRef) -> Result<(), Self::Error> {
        self.revoked.lock().unwrap().push(credential.clone());
        Ok(())
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

fn source_scoped_credential_id() -> CredentialId {
    CredentialId::new("openbao:tenant-1:github:webhook_secret").unwrap()
}

fn runtime_key() -> RuntimeIntegrationKey {
    RuntimeIntegrationKey::new(SourceKind::GitHub, &integration_id())
}

fn source_runtime_key() -> RuntimeIntegrationKey {
    RuntimeIntegrationKey::for_source(SourceKind::GitHub)
}

fn credential_ref(version: u64) -> CredentialRef {
    CredentialRef::new(
        credential_id(),
        CredentialVersion::new(version).unwrap(),
        &scope(),
        CredentialKind::WebhookSecret,
    )
}

fn destroy_reason() -> SecretDestroyReason {
    SecretDestroyReason::new("credential lifecycle cleanup").unwrap()
}

fn put_command(value: &str) -> PutCredential {
    PutCredential::new(
        credential_id(),
        scope(),
        CredentialKind::WebhookSecret,
        SecretString::new(value).unwrap(),
    )
}

fn source_scoped_put_command(value: &str) -> PutCredential {
    PutCredential::new(
        source_scoped_credential_id(),
        CredentialScope::source(owner_id(), SourceKind::GitHub),
        CredentialKind::WebhookSecret,
        SecretString::new(value).unwrap(),
    )
}

fn stored_event(event: &v1::CredentialEvent, id: u128) -> Event {
    Event {
        id: EventId::new(uuid::Uuid::from_u128(id)),
        r#type: event.event_type().unwrap().to_string(),
        content: event.encode().unwrap(),
        headers: Headers::empty(),
    }
}

fn payload_contains(payload: &[u8], needle: &str) -> bool {
    payload.windows(needle.len()).any(|window| window == needle.as_bytes())
}

fn write_requested_event() -> v1::CredentialEvent {
    v1::CredentialEvent {
        event: Some(write_requested_to_proto(&credential_id(), &scope(), CredentialKind::WebhookSecret).into()),
    }
}

fn activated_event(metadata: CredentialMetadata) -> v1::CredentialEvent {
    v1::CredentialEvent {
        event: Some(activated_to_proto(&metadata).into()),
    }
}

fn rotation_requested_event(version: u64) -> v1::CredentialEvent {
    v1::CredentialEvent {
        event: Some(rotation_requested_to_proto(&credential_ref(version)).into()),
    }
}

struct OpenBaoServer {
    _container: ContainerAsync<GenericImage>,
    address: String,
}

impl OpenBaoServer {
    async fn start() -> Self {
        let container = GenericImage::new(OPENBAO_IMAGE, OPENBAO_IMAGE_TAG)
            .with_wait_for(WaitFor::message_on_stdout(
                "Development mode should NOT be used in production",
            ))
            .with_exposed_port(OPENBAO_PORT.tcp())
            .with_cmd(vec![
                "server",
                "-dev",
                "-dev-root-token-id=dev-only-token",
                "-dev-listen-address=0.0.0.0:8200",
            ])
            .start()
            .await
            .expect("start OpenBao testcontainer");
        let host = container.get_host().await.expect("get OpenBao testcontainer host");
        let port = container
            .get_host_port_ipv4(OPENBAO_PORT)
            .await
            .expect("get OpenBao testcontainer port");
        Self {
            _container: container,
            address: format!("http://{host}:{port}"),
        }
    }

    fn store(&self) -> OpenBaoSecretStore {
        OpenBaoSecretStore::new(&self.address, OPENBAO_DEV_ROOT_TOKEN).unwrap()
    }
}

#[test]
fn recovery_plan_builds_write_activation_command_for_pending_write() {
    let state = evolve(initial_state(), &write_requested_event()).unwrap();

    let command = activation_recovery_command(&credential_id(), &state, None).unwrap();

    assert_eq!(
        command,
        Some(CredentialActivationRecoveryCommand::Write(
            RecoverCredentialWriteActivation::new(credential_ref(1))
        ))
    );
}

#[test]
fn recovery_plan_builds_write_activation_from_the_pending_scope_for_any_backend() {
    let credential_id = CredentialId::new("memory:github/primary:webhook_secret:1").unwrap();
    let state = state_v1::CredentialStateSnapshot {
        state: Some(pending_write_to_proto_state(&credential_id, &scope(), CredentialKind::WebhookSecret).into()),
    };

    let command = activation_recovery_command(&credential_id, &state, None).unwrap();

    assert_eq!(
        command,
        Some(CredentialActivationRecoveryCommand::Write(
            RecoverCredentialWriteActivation::new(CredentialRef::new(
                credential_id,
                CredentialVersion::initial(),
                &scope(),
                CredentialKind::WebhookSecret,
            ))
        ))
    );
}

#[test]
fn recovery_plan_rejects_a_pending_write_from_another_stream() {
    let state = evolve(initial_state(), &write_requested_event()).unwrap();
    let stream_id = source_scoped_credential_id();

    let error = activation_recovery_command(&stream_id, &state, None).unwrap_err();

    assert!(matches!(
        error,
        CredentialActivationRecoveryPlanError::PendingWriteMismatch { expected, actual }
            if expected == stream_id && actual == credential_id()
    ));
}

#[test]
fn recovery_plan_builds_rotation_activation_command_for_pending_rotation() {
    let active = CredentialMetadata::new(
        credential_ref(1),
        CredentialStatus::Active,
        StorageBackend::OpenBao,
        CredentialFingerprint::new("fingerprint").unwrap(),
    );
    let state = [
        write_requested_event(),
        activated_event(active),
        rotation_requested_event(1),
    ]
    .into_iter()
    .try_fold(initial_state(), |state, event| evolve(state, &event))
    .unwrap();

    let command = activation_recovery_command(&credential_id(), &state, None).unwrap();

    assert_eq!(
        command,
        Some(CredentialActivationRecoveryCommand::Rotation(
            RecoverCredentialRotationActivation::new(credential_ref(2))
        ))
    );
}

#[test]
fn recovery_plan_skips_completed_state() {
    let active = CredentialMetadata::new(
        credential_ref(1),
        CredentialStatus::Active,
        StorageBackend::OpenBao,
        CredentialFingerprint::new("fingerprint").unwrap(),
    );
    let state = [write_requested_event(), activated_event(active)]
        .into_iter()
        .try_fold(initial_state(), |state, event| evolve(state, &event))
        .unwrap();

    let command = activation_recovery_command(&credential_id(), &state, None).unwrap();

    assert_eq!(command, None);
}

#[tokio::test]
async fn put_records_request_writes_secret_and_activates_metadata() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());

    let outcome = handler.put(put_command("super-secret")).await.unwrap();

    assert_eq!(outcome.stream_position(), position(2));
    assert!(matches!(
        outcome.state().state.as_ref(),
        Some(CredentialStateSnapshotCase::Active(_))
    ));
    let state = outcome.into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (metadata, _previous_versions) = decode_active_state("active", active).unwrap();
    assert_eq!(metadata.reference().id(), &credential_id());
    assert_eq!(metadata.storage_backend(), StorageBackend::OpenBao);
    assert_eq!(
        secrets
            .get(metadata.reference())
            .await
            .unwrap()
            .as_plaintext()
            .unwrap()
            .as_str(),
        "super-secret"
    );
    assert_eq!(
        events.write_preconditions(),
        [
            StreamWritePrecondition::NoStream,
            StreamWritePrecondition::At(position(1))
        ]
    );
    assert_eq!(events.decoded_events().len(), 2);
    for event in events.events() {
        assert!(!payload_contains(&event.event.content, "super-secret"));
    }
}

#[tokio::test]
async fn put_records_write_failure_when_secret_store_rejects_the_side_effect() {
    let events = HandlerTestStreamStore::default();
    let handler = CredentialHandler::new(events.clone(), FailingSecretStore);

    let error = handler.put(put_command("ignored-secret")).await.unwrap_err();

    assert!(matches!(error, CredentialHandlerError::SecretWrite { .. }));
    let decoded = events.decoded_events();
    assert_eq!(decoded.len(), 2);
    assert!(matches!(decoded[0].event, Some(CredentialEventCase::WriteRequested(_))));
    assert!(matches!(decoded[1].event, Some(CredentialEventCase::WriteFailed(_))));
    assert_eq!(
        events.write_preconditions(),
        [
            StreamWritePrecondition::NoStream,
            StreamWritePrecondition::At(position(1))
        ]
    );
}

#[tokio::test]
async fn put_retry_resumes_pending_write_after_activation_append_failure() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    events.fail_append_at(2);

    let error = handler.put(put_command("first-secret")).await.unwrap_err();

    assert!(matches!(error, CredentialHandlerError::ActivateWrite { .. }));
    assert_eq!(events.decoded_events().len(), 1);

    let outcome = handler.put(put_command("retry-secret")).await.unwrap();

    assert_eq!(outcome.stream_position(), position(2));
    let state = outcome.into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (metadata, _previous_versions) = decode_active_state("active", active).unwrap();
    assert_eq!(metadata.reference().version().get(), 2);
    assert_eq!(
        secrets
            .get(metadata.reference())
            .await
            .unwrap()
            .as_plaintext()
            .unwrap()
            .as_str(),
        "retry-secret"
    );
    assert_eq!(events.decoded_events().len(), 2);
    assert_eq!(
        events.write_preconditions(),
        [
            StreamWritePrecondition::NoStream,
            StreamWritePrecondition::At(position(1)),
            StreamWritePrecondition::At(position(1))
        ]
    );
}

#[tokio::test]
async fn put_retry_activates_credential_after_secret_store_failure() {
    let events = HandlerTestStreamStore::default();

    let error = CredentialHandler::new(events.clone(), FailingSecretStore)
        .put(put_command("first-secret"))
        .await
        .unwrap_err();

    assert!(matches!(error, CredentialHandlerError::SecretWrite { .. }));
    let decoded = events.decoded_events();
    assert_eq!(decoded.len(), 2);
    assert!(matches!(decoded[1].event, Some(CredentialEventCase::WriteFailed(_))));

    let secrets = MockOpenBaoSecretStore::default();
    let outcome = CredentialHandler::new(events.clone(), secrets.clone())
        .put(put_command("retry-secret"))
        .await
        .unwrap();

    assert_eq!(outcome.stream_position(), position(4));
    let state = outcome.into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (metadata, _previous_versions) = decode_active_state("active", active).unwrap();
    assert_eq!(metadata.reference().id(), &credential_id());
    assert_eq!(
        secrets
            .get(metadata.reference())
            .await
            .unwrap()
            .as_plaintext()
            .unwrap()
            .as_str(),
        "retry-secret"
    );
    let decoded = events.decoded_events();
    assert_eq!(decoded.len(), 4);
    assert!(matches!(decoded[2].event, Some(CredentialEventCase::WriteRequested(_))));
    assert!(matches!(decoded[3].event, Some(CredentialEventCase::Activated(_))));
    assert_eq!(
        events.write_preconditions(),
        [
            StreamWritePrecondition::NoStream,
            StreamWritePrecondition::At(position(1)),
            StreamWritePrecondition::At(position(2)),
            StreamWritePrecondition::At(position(3))
        ]
    );
}

#[derive(Clone)]
struct GatedSecretStore {
    inner: MockOpenBaoSecretStore,
    writes: Arc<std::sync::atomic::AtomicUsize>,
    release: Arc<tokio::sync::Notify>,
}

impl GatedSecretStore {
    fn new() -> Self {
        Self {
            inner: MockOpenBaoSecretStore::default(),
            writes: Arc::default(),
            release: Arc::default(),
        }
    }

    async fn gate_first_write(&self) {
        if self.writes.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            self.release.notified().await;
        }
    }

    fn writes(&self) -> usize {
        self.writes.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl SecretStorePut for GatedSecretStore {
    type Error = SecretStoreError;

    async fn put(
        &self,
        scope: CredentialScope,
        kind: CredentialKind,
        value: SecretString,
    ) -> Result<CredentialRef, Self::Error> {
        self.gate_first_write().await;
        self.inner.put(scope, kind, value).await
    }
}

impl SecretStoreRotate for GatedSecretStore {
    type Error = SecretStoreError;

    async fn rotate(&self, credential: &CredentialRef, value: SecretString) -> Result<CredentialRef, Self::Error> {
        self.gate_first_write().await;
        self.inner.rotate(credential, value).await
    }
}

impl SecretStoreMetadata for GatedSecretStore {
    type Error = SecretStoreError;

    async fn metadata(&self, credential: &CredentialRef) -> Result<CredentialMetadata, Self::Error> {
        self.inner.metadata(credential).await
    }
}

#[tokio::test]
async fn concurrent_put_with_another_secret_is_rejected_instead_of_overwriting_the_store() {
    let events = HandlerTestStreamStore::default();
    let secrets = GatedSecretStore::new();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());

    let first = handler.put(put_command("first-secret"));
    let second = handler.put(put_command("second-secret"));
    let releaser = async {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        secrets.release.notify_one();
    };
    let (first, second, ()) = tokio::join!(first, second, releaser);

    let state = first.unwrap().into_state();
    assert!(matches!(second, Err(CredentialHandlerError::RequestWrite { .. })));
    assert_eq!(secrets.writes(), 1);
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (metadata, _) = decode_active_state("active", active).unwrap();
    assert_eq!(
        secrets
            .inner
            .get(metadata.reference())
            .await
            .unwrap()
            .as_plaintext()
            .unwrap()
            .as_str(),
        "first-secret"
    );
}

#[tokio::test]
async fn concurrent_rotation_with_another_secret_is_rejected_instead_of_overwriting_the_store() {
    let events = HandlerTestStreamStore::default();
    let secrets = GatedSecretStore::new();
    secrets.writes.store(1, std::sync::atomic::Ordering::SeqCst);
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let credential_ref = active_metadata.reference().clone();
    secrets.writes.store(0, std::sync::atomic::Ordering::SeqCst);

    let first = handler.rotate(RotateCredential::new(
        credential_ref.clone(),
        SecretString::new("first-rotation").unwrap(),
    ));
    let second = handler.rotate(RotateCredential::new(
        credential_ref.clone(),
        SecretString::new("second-rotation").unwrap(),
    ));
    let releaser = async {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        secrets.release.notify_one();
    };
    let (first, second, ()) = tokio::join!(first, second, releaser);

    first.unwrap();
    assert!(matches!(second, Err(CredentialHandlerError::RequestRotation { .. })));
    assert_eq!(secrets.writes(), 1);
}

#[tokio::test]
async fn recover_write_activation_records_missing_activation_without_plaintext() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    events.fail_append_at(2);

    let error = handler.put(put_command("first-secret")).await.unwrap_err();

    assert!(matches!(error, CredentialHandlerError::ActivateWrite { .. }));
    assert_eq!(events.decoded_events().len(), 1);
    assert_eq!(
        secrets
            .get(&credential_ref(1))
            .await
            .unwrap()
            .as_plaintext()
            .unwrap()
            .as_str(),
        "first-secret"
    );

    let outcome = handler
        .recover_write_activation(RecoverCredentialWriteActivation::new(credential_ref(1)))
        .await
        .unwrap();

    assert_eq!(outcome.stream_position(), position(2));
    let state = outcome.into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (metadata, _previous_versions) = decode_active_state("active", active).unwrap();
    assert_eq!(metadata.reference(), &credential_ref(1));
    assert_eq!(events.decoded_events().len(), 2);
    assert_eq!(
        events.write_preconditions(),
        [
            StreamWritePrecondition::NoStream,
            StreamWritePrecondition::At(position(1)),
            StreamWritePrecondition::At(position(1))
        ]
    );
    for event in events.events() {
        assert!(!payload_contains(&event.event.content, "first-secret"));
    }
}

#[tokio::test]
async fn rotate_records_request_rotates_secret_and_activates_metadata() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();

    let outcome = handler
        .rotate(RotateCredential::new(
            active_metadata.reference().clone(),
            SecretString::new("rotated-secret").unwrap(),
        ))
        .await
        .unwrap();

    assert_eq!(outcome.stream_position(), position(4));
    let state = outcome.into_state();
    let Some(CredentialStateSnapshotCase::Active(rotated)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (rotated_metadata, rotated_previous_versions) = decode_active_state("active", rotated).unwrap();
    assert_eq!(rotated_metadata.reference().version().get(), 2);
    assert_eq!(rotated_previous_versions, vec![active_metadata.reference().clone()]);
    assert_eq!(
        secrets
            .get(rotated_metadata.reference())
            .await
            .unwrap()
            .as_plaintext()
            .unwrap()
            .as_str(),
        "rotated-secret"
    );
    assert_eq!(
        events.write_preconditions(),
        [
            StreamWritePrecondition::NoStream,
            StreamWritePrecondition::At(position(1)),
            StreamWritePrecondition::At(position(2)),
            StreamWritePrecondition::At(position(3))
        ]
    );
    let decoded = events.decoded_events();
    assert!(matches!(
        decoded[2].event,
        Some(CredentialEventCase::RotationRequested(_))
    ));
    assert!(matches!(decoded[3].event, Some(CredentialEventCase::Rotated(_))));
    for event in events.events() {
        assert!(!payload_contains(&event.event.content, "rotated-secret"));
    }
}

#[derive(Clone, Default)]
struct CountingRotateSecretStore {
    inner: MockOpenBaoSecretStore,
    rotations: Arc<std::sync::atomic::AtomicUsize>,
    metadata_outage: Arc<std::sync::atomic::AtomicBool>,
}

impl SecretStorePut for CountingRotateSecretStore {
    type Error = SecretStoreError;

    async fn put(
        &self,
        scope: CredentialScope,
        kind: CredentialKind,
        value: SecretString,
    ) -> Result<CredentialRef, Self::Error> {
        self.inner.put(scope, kind, value).await
    }
}

impl SecretStoreRotate for CountingRotateSecretStore {
    type Error = SecretStoreError;

    async fn rotate(&self, credential: &CredentialRef, value: SecretString) -> Result<CredentialRef, Self::Error> {
        self.rotations.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.rotate(credential, value).await
    }
}

impl SecretStoreMetadata for CountingRotateSecretStore {
    type Error = SecretStoreError;

    async fn metadata(&self, credential: &CredentialRef) -> Result<CredentialMetadata, Self::Error> {
        if self.metadata_outage.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(SecretStoreError::BackendUnavailable {
                backend: StorageBackend::OpenBao,
                message: "metadata read timed out".to_string(),
            });
        }
        self.inner.metadata(credential).await
    }
}

#[tokio::test]
async fn rotate_retry_surfaces_metadata_outage_without_rotating_again() {
    let events = HandlerTestStreamStore::default();
    let secrets = CountingRotateSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let credential_ref = active_metadata.reference().clone();
    events.fail_append_at(4);
    handler
        .rotate(RotateCredential::new(
            credential_ref.clone(),
            SecretString::new("rotated-secret").unwrap(),
        ))
        .await
        .unwrap_err();
    secrets.metadata_outage.store(true, std::sync::atomic::Ordering::SeqCst);

    let error = handler
        .rotate(RotateCredential::new(
            credential_ref.clone(),
            SecretString::new("rotated-secret").unwrap(),
        ))
        .await
        .unwrap_err();

    assert!(matches!(error, CredentialHandlerError::SecretMetadata { .. }));
    assert_eq!(secrets.rotations.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        !events
            .decoded_events()
            .iter()
            .any(|event| matches!(event.event, Some(CredentialEventCase::RotationFailed(_))))
    );

    secrets
        .metadata_outage
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let state = handler
        .rotate(RotateCredential::new(
            credential_ref.clone(),
            SecretString::new("rotated-secret").unwrap(),
        ))
        .await
        .unwrap()
        .into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (metadata, _) = decode_active_state("active", active).unwrap();
    assert_eq!(metadata.reference(), &credential_ref.next_version());
    assert_eq!(secrets.rotations.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rotate_retry_activates_the_version_the_store_already_wrote() {
    let events = HandlerTestStreamStore::default();
    let secrets = CountingRotateSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let credential_ref = active_metadata.reference().clone();
    events.fail_append_at(4);
    let error = handler
        .rotate(RotateCredential::new(
            credential_ref.clone(),
            SecretString::new("rotated-secret").unwrap(),
        ))
        .await
        .unwrap_err();
    assert!(matches!(error, CredentialHandlerError::ActivateRotation { .. }));

    let outcome = handler
        .rotate(RotateCredential::new(
            credential_ref.clone(),
            SecretString::new("rotated-secret").unwrap(),
        ))
        .await
        .unwrap();

    assert_eq!(secrets.rotations.load(std::sync::atomic::Ordering::SeqCst), 1);
    let state = outcome.into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (metadata, _) = decode_active_state("active", active).unwrap();
    assert_eq!(metadata.reference(), &credential_ref.next_version());
    assert!(
        !events
            .decoded_events()
            .iter()
            .any(|event| matches!(event.event, Some(CredentialEventCase::RotationFailed(_))))
    );
}

#[tokio::test]
async fn rotate_records_rotation_failure_when_secret_store_rejects_the_side_effect() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let put_handler = CredentialHandler::new(events.clone(), secrets);
    let state = put_handler
        .put(put_command("initial-secret"))
        .await
        .unwrap()
        .into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let rotate_handler = CredentialHandler::new(events.clone(), FailingRotateSecretStore);

    let error = rotate_handler
        .rotate(RotateCredential::new(
            active_metadata.reference().clone(),
            SecretString::new("ignored-secret").unwrap(),
        ))
        .await
        .unwrap_err();

    assert!(matches!(error, CredentialHandlerError::SecretRotate { .. }));
    let decoded = events.decoded_events();
    assert_eq!(decoded.len(), 4);
    assert!(matches!(
        decoded[2].event,
        Some(CredentialEventCase::RotationRequested(_))
    ));
    assert!(matches!(decoded[3].event, Some(CredentialEventCase::RotationFailed(_))));
    assert_eq!(
        events.write_preconditions(),
        [
            StreamWritePrecondition::NoStream,
            StreamWritePrecondition::At(position(1)),
            StreamWritePrecondition::At(position(2)),
            StreamWritePrecondition::At(position(3))
        ]
    );
}

#[tokio::test]
async fn recover_rotation_activation_records_missing_rotation_without_plaintext() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let rotated_ref = active_metadata.reference().next_version();
    events.fail_append_at(4);

    let error = handler
        .rotate(RotateCredential::new(
            active_metadata.reference().clone(),
            SecretString::new("rotated-secret").unwrap(),
        ))
        .await
        .unwrap_err();

    assert!(matches!(error, CredentialHandlerError::ActivateRotation { .. }));
    assert_eq!(events.decoded_events().len(), 3);
    assert_eq!(
        secrets
            .get(&rotated_ref)
            .await
            .unwrap()
            .as_plaintext()
            .unwrap()
            .as_str(),
        "rotated-secret"
    );

    let outcome = handler
        .recover_rotation_activation(RecoverCredentialRotationActivation::new(rotated_ref.clone()))
        .await
        .unwrap();

    assert_eq!(outcome.stream_position(), position(4));
    let state = outcome.into_state();
    let Some(CredentialStateSnapshotCase::Active(rotated)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (rotated_metadata, rotated_previous_versions) = decode_active_state("active", rotated).unwrap();
    assert_eq!(rotated_metadata.reference(), &rotated_ref);
    assert_eq!(rotated_previous_versions, vec![active_metadata.reference().clone()]);
    assert_eq!(events.decoded_events().len(), 4);
    assert_eq!(
        events.write_preconditions(),
        [
            StreamWritePrecondition::NoStream,
            StreamWritePrecondition::At(position(1)),
            StreamWritePrecondition::At(position(2)),
            StreamWritePrecondition::At(position(3)),
            StreamWritePrecondition::At(position(3))
        ]
    );
    for event in events.events() {
        assert!(!payload_contains(&event.event.content, "rotated-secret"));
    }
}

#[tokio::test]
async fn revoke_revokes_secret_and_records_state() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let credential_ref = active_metadata.reference().clone();

    let outcome = handler
        .revoke(RevokeStoredCredential::new(credential_ref.clone()))
        .await
        .unwrap();

    assert_eq!(outcome.stream_position(), position(3));
    let state = outcome.into_state();
    assert!(matches!(
        state.state.as_ref(),
        Some(CredentialStateSnapshotCase::Revoked(_))
    ));
    assert!(matches!(
        secrets.get(&credential_ref).await,
        Err(SecretStoreError::Unreadable { .. })
    ));
    let decoded = events.decoded_events();
    assert!(matches!(decoded[2].event, Some(CredentialEventCase::Revoked(_))));
    assert_eq!(
        events.write_preconditions(),
        [
            StreamWritePrecondition::NoStream,
            StreamWritePrecondition::At(position(1)),
            StreamWritePrecondition::At(position(2))
        ]
    );
}

#[tokio::test]
async fn revoke_retry_records_state_after_append_failure() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let credential_ref = active_metadata.reference().clone();
    events.fail_append_at(3);

    let error = handler
        .revoke(RevokeStoredCredential::new(credential_ref.clone()))
        .await
        .unwrap_err();

    assert!(matches!(error, CredentialHandlerError::Revoke { .. }));
    assert_eq!(events.decoded_events().len(), 2);
    assert!(matches!(
        secrets.get(&credential_ref).await,
        Err(SecretStoreError::Unreadable { .. })
    ));

    let outcome = handler
        .revoke(RevokeStoredCredential::new(credential_ref))
        .await
        .unwrap();

    assert_eq!(outcome.stream_position(), position(3));
    let state = outcome.into_state();
    assert!(matches!(
        state.state.as_ref(),
        Some(CredentialStateSnapshotCase::Revoked(_))
    ));
    assert_eq!(events.decoded_events().len(), 3);
    assert_eq!(
        events.write_preconditions(),
        [
            StreamWritePrecondition::NoStream,
            StreamWritePrecondition::At(position(1)),
            StreamWritePrecondition::At(position(2)),
            StreamWritePrecondition::At(position(2))
        ]
    );
}

#[derive(Clone)]
struct RotationRacingRevokeSecretStore {
    events: HandlerTestStreamStore,
    revoked: Arc<Mutex<Vec<CredentialRef>>>,
}

impl SecretStoreRevoke for RotationRacingRevokeSecretStore {
    type Error = SecretStoreError;

    async fn revoke(&self, credential: &CredentialRef) -> Result<(), Self::Error> {
        self.revoked.lock().unwrap().push(credential.clone());
        let current = current_position(&self.events.events(), credential.id().as_str());
        self.events
            .append_stream(AppendStreamRequest {
                stream_id: credential.id().as_str(),
                stream_write_precondition: StreamWritePrecondition::At(current.unwrap()),
                events: vec![stored_event(&rotation_requested_event(credential.version().get()), 900)],
            })
            .await
            .unwrap();
        Ok(())
    }
}

#[tokio::test]
async fn revoke_completes_when_a_rotation_request_lands_after_the_secret_was_disabled() {
    let events = HandlerTestStreamStore::default();
    let handler = CredentialHandler::new(events.clone(), MockOpenBaoSecretStore::default());
    let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let credential_ref = active_metadata.reference().clone();
    let secrets = RotationRacingRevokeSecretStore {
        events: events.clone(),
        revoked: Arc::default(),
    };

    let outcome = CredentialHandler::new(events.clone(), secrets.clone())
        .revoke(RevokeStoredCredential::new(credential_ref.clone()))
        .await
        .unwrap();

    assert!(matches!(
        outcome.state().state.as_ref(),
        Some(CredentialStateSnapshotCase::Revoked(_))
    ));
    assert_eq!(secrets.revoked.lock().unwrap().clone(), [credential_ref]);
    let decoded = events.decoded_events();
    assert!(matches!(
        decoded[decoded.len() - 2].event,
        Some(CredentialEventCase::RotationRequested(_))
    ));
    assert!(matches!(
        decoded[decoded.len() - 1].event,
        Some(CredentialEventCase::Revoked(_))
    ));
}

#[tokio::test]
async fn revoke_revokes_a_credential_whose_rotation_is_pending() {
    let events = HandlerTestStreamStore::default();
    let handler = CredentialHandler::new(events.clone(), MockOpenBaoSecretStore::default());
    let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let credential_ref = active_metadata.reference().clone();
    events.fail_append_at(4);
    handler
        .rotate(RotateCredential::new(
            credential_ref.clone(),
            SecretString::new("rotated-secret").unwrap(),
        ))
        .await
        .unwrap_err();
    let secrets = RecordingRevokeSecretStore::default();

    let outcome = CredentialHandler::new(events.clone(), secrets.clone())
        .revoke(RevokeStoredCredential::new(credential_ref.clone()))
        .await
        .unwrap();

    assert!(matches!(
        outcome.state().state.as_ref(),
        Some(CredentialStateSnapshotCase::Revoked(_))
    ));
    assert_eq!(secrets.revoked(), [credential_ref]);
}

#[tokio::test]
async fn revoke_is_idempotent_for_an_already_revoked_credential() {
    let events = HandlerTestStreamStore::default();
    let handler = CredentialHandler::new(events.clone(), MockOpenBaoSecretStore::default());
    let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let credential_ref = active_metadata.reference().clone();
    let first = handler
        .revoke(RevokeStoredCredential::new(credential_ref.clone()))
        .await
        .unwrap();
    let secrets = RecordingRevokeSecretStore::default();
    let revoking_handler = CredentialHandler::new(events.clone(), secrets.clone());

    let second = revoking_handler
        .revoke(RevokeStoredCredential::new(credential_ref.clone()))
        .await
        .unwrap();

    assert_eq!(second.stream_position(), first.stream_position());
    let Some(CredentialStateSnapshotCase::Revoked(revoked)) = second.state().state.as_ref() else {
        panic!("expected revoked credential");
    };
    assert_eq!(decode_revoked_state(revoked).unwrap(), credential_ref);
    assert!(secrets.revoked().is_empty());
    assert_eq!(events.decoded_events().len(), 3);
}

#[tokio::test]
async fn revoke_rejects_an_already_revoked_credential_with_a_different_ref() {
    let events = HandlerTestStreamStore::default();
    let handler = CredentialHandler::new(events.clone(), MockOpenBaoSecretStore::default());
    let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let credential_ref = active_metadata.reference().clone();
    handler
        .revoke(RevokeStoredCredential::new(credential_ref.clone()))
        .await
        .unwrap();
    let secrets = RecordingRevokeSecretStore::default();
    let revoking_handler = CredentialHandler::new(events.clone(), secrets.clone());
    let other_ref = credential_ref.next_version();

    let error = revoking_handler
        .revoke(RevokeStoredCredential::new(other_ref))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        CredentialHandlerError::Revoke {
            source: CommandError::Decide(CredentialDecideError::CredentialNotActive { credential_id }),
        } if credential_id == *credential_ref.id()
    ));
    assert!(secrets.revoked().is_empty());
    assert_eq!(events.decoded_events().len(), 3);
}

#[tokio::test]
async fn destroy_destroys_secret_and_records_state() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let credential_ref = active_metadata.reference().clone();
    handler
        .revoke(RevokeStoredCredential::new(credential_ref.clone()))
        .await
        .unwrap();

    let outcome = handler
        .destroy(DestroyStoredCredential::new(credential_ref.clone(), destroy_reason()))
        .await
        .unwrap();

    assert_eq!(outcome.stream_position(), position(5));
    let state = outcome.into_state();
    assert!(matches!(
        state.state.as_ref(),
        Some(CredentialStateSnapshotCase::Destroyed(_))
    ));
    assert!(matches!(
        secrets.get(&credential_ref).await,
        Err(SecretStoreError::Unreadable { .. })
    ));
    let decoded = events.decoded_events();
    assert_eq!(decoded.len(), 5);
    assert!(matches!(
        decoded[3].event,
        Some(CredentialEventCase::DestroyRequested(_))
    ));
    assert!(matches!(decoded[4].event, Some(CredentialEventCase::Destroyed(_))));
}

#[tokio::test]
async fn put_after_revoke_and_destroy_restarts_the_credential_on_the_next_version() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let runtime_credentials = RuntimeCredentialRegistry::default();
    let resolver = runtime_credentials.resolver(secrets.clone());
    let handler = CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials);
    let state = handler.put(put_command("first-secret")).await.unwrap().into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let destroyed_ref = active_metadata.reference().clone();
    handler
        .revoke(RevokeStoredCredential::new(destroyed_ref.clone()))
        .await
        .unwrap();
    handler
        .destroy(DestroyStoredCredential::new(destroyed_ref.clone(), destroy_reason()))
        .await
        .unwrap();

    let outcome = handler.put(put_command("second-secret")).await.unwrap();

    assert_eq!(outcome.stream_position(), position(7));
    let state = outcome.into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    assert_eq!(active_metadata.reference(), &destroyed_ref.next_version());
    assert!(matches!(
        secrets.get(&destroyed_ref).await,
        Err(SecretStoreError::Unreadable { .. })
    ));
    let resolved = resolver
        .resolve_plaintext(&runtime_key(), CredentialKind::WebhookSecret)
        .await
        .unwrap();
    assert_eq!(resolved.as_str(), "second-secret");
}

#[test]
fn recovery_after_destroy_targets_the_version_following_the_destroyed_one() {
    let destroyed_ref = credential_ref(3);
    let state = state_v1::CredentialStateSnapshot {
        state: Some(pending_write_to_proto_state(&credential_id(), &scope(), CredentialKind::WebhookSecret).into()),
    };

    let command = activation_recovery_command(&credential_id(), &state, Some(destroyed_ref.version())).unwrap();

    assert_eq!(
        command,
        Some(CredentialActivationRecoveryCommand::Write(
            RecoverCredentialWriteActivation::new(credential_ref(4))
        ))
    );
}

#[tokio::test]
async fn destroy_records_cleanup_failed_state_when_secret_store_fails() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let credential_ref = active_metadata.reference().clone();
    handler
        .revoke(RevokeStoredCredential::new(credential_ref.clone()))
        .await
        .unwrap();
    let failing_handler = CredentialHandler::new(events.clone(), FailingDestroySecretStore);

    let error = failing_handler
        .destroy(DestroyStoredCredential::new(credential_ref.clone(), destroy_reason()))
        .await
        .unwrap_err();

    assert!(matches!(error, CredentialHandlerError::SecretDestroy { .. }));
    let decoded = events.decoded_events();
    assert_eq!(decoded.len(), 5);
    assert!(matches!(
        decoded[3].event,
        Some(CredentialEventCase::DestroyRequested(_))
    ));
    assert!(matches!(decoded[4].event, Some(CredentialEventCase::DestroyFailed(_))));
}

#[tokio::test]
async fn destroy_retries_after_cleanup_failure() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let credential_ref = active_metadata.reference().clone();
    handler
        .revoke(RevokeStoredCredential::new(credential_ref.clone()))
        .await
        .unwrap();
    let failing_handler = CredentialHandler::new(events.clone(), FailingDestroySecretStore);
    failing_handler
        .destroy(DestroyStoredCredential::new(credential_ref.clone(), destroy_reason()))
        .await
        .unwrap_err();

    let outcome = handler
        .destroy(DestroyStoredCredential::new(credential_ref.clone(), destroy_reason()))
        .await
        .unwrap();

    assert!(matches!(
        outcome.into_state().state.as_ref(),
        Some(CredentialStateSnapshotCase::Destroyed(_))
    ));
    let decoded = events.decoded_events();
    assert_eq!(decoded.len(), 7);
    assert!(matches!(
        decoded[5].event,
        Some(CredentialEventCase::DestroyRequested(_))
    ));
    assert!(matches!(decoded[6].event, Some(CredentialEventCase::Destroyed(_))));
}

#[tokio::test]
async fn destroy_rejects_credential_that_is_not_revoked_or_expired() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets);

    let error = handler
        .destroy(DestroyStoredCredential::new(credential_ref(1), destroy_reason()))
        .await
        .unwrap_err();

    assert!(matches!(error, CredentialHandlerError::RequestDestroy { .. }));
    assert!(events.decoded_events().is_empty());
}

#[tokio::test]
async fn destroy_is_idempotent_for_already_destroyed_credential() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let handler = CredentialHandler::new(events.clone(), secrets.clone());
    let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let credential_ref = active_metadata.reference().clone();
    handler
        .revoke(RevokeStoredCredential::new(credential_ref.clone()))
        .await
        .unwrap();
    let first = handler
        .destroy(DestroyStoredCredential::new(credential_ref.clone(), destroy_reason()))
        .await
        .unwrap();
    assert_eq!(first.stream_position(), position(5));

    let second = handler
        .destroy(DestroyStoredCredential::new(credential_ref.clone(), destroy_reason()))
        .await
        .unwrap();

    assert_eq!(second.stream_position(), position(5));
    assert!(matches!(
        second.into_state().state.as_ref(),
        Some(CredentialStateSnapshotCase::Destroyed(_))
    ));
    assert_eq!(events.decoded_events().len(), 5);
}

#[tokio::test]
async fn runtime_handler_put_updates_runtime_projection() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let runtime_credentials = RuntimeCredentialRegistry::default();
    let resolver = runtime_credentials.resolver(secrets.clone());
    let handler = CredentialRuntimeHandler::new(events.clone(), secrets, runtime_credentials);

    let outcome = handler.put(put_command("super-secret")).await.unwrap();

    assert_eq!(outcome.stream_position(), position(2));
    assert!(matches!(
        outcome.state().state.as_ref(),
        Some(CredentialStateSnapshotCase::Active(_))
    ));
    assert_eq!(
        resolver
            .resolve_plaintext(&runtime_key(), CredentialKind::WebhookSecret)
            .await
            .unwrap()
            .as_str(),
        "super-secret"
    );
}

#[tokio::test]
async fn runtime_handler_put_source_scoped_credential_updates_source_projection() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let runtime_credentials = RuntimeCredentialRegistry::default();
    let resolver = runtime_credentials.resolver(secrets.clone());
    let handler = CredentialRuntimeHandler::new(events.clone(), secrets, runtime_credentials);

    let outcome = handler.put(source_scoped_put_command("source-secret")).await.unwrap();

    assert_eq!(outcome.stream_position(), position(2));
    assert!(matches!(
        outcome.state().state.as_ref(),
        Some(CredentialStateSnapshotCase::Active(_))
    ));
    assert_eq!(
        resolver
            .resolve_plaintext(&source_runtime_key(), CredentialKind::WebhookSecret)
            .await
            .unwrap()
            .as_str(),
        "source-secret"
    );
}

#[tokio::test]
async fn runtime_handler_recover_write_activation_updates_runtime_projection() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let runtime_credentials = RuntimeCredentialRegistry::default();
    let resolver = runtime_credentials.resolver(secrets.clone());
    let handler = CredentialRuntimeHandler::new(events.clone(), secrets, runtime_credentials);
    events.fail_append_at(2);

    let error = handler.put(put_command("super-secret")).await.unwrap_err();

    assert!(matches!(
        error,
        CredentialRuntimeHandlerError::Command {
            source: CredentialHandlerError::ActivateWrite { .. }
        }
    ));

    let outcome = handler
        .recover_write_activation(RecoverCredentialWriteActivation::new(credential_ref(1)))
        .await
        .unwrap();

    assert_eq!(outcome.stream_position(), position(2));
    assert_eq!(
        resolver
            .resolve_plaintext(&runtime_key(), CredentialKind::WebhookSecret)
            .await
            .unwrap()
            .as_str(),
        "super-secret"
    );
}

#[tokio::test]
async fn runtime_handler_revoke_removes_projection_and_invalidates_cached_secret() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let runtime_credentials = RuntimeCredentialRegistry::default();
    let resolver = runtime_credentials.resolver(secrets.clone());
    let handler = CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials.clone());
    let outcome = handler.put(put_command("super-secret")).await.unwrap();
    let state = outcome.into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let credential_ref = active_metadata.reference().clone();
    assert_eq!(
        resolver
            .resolve_plaintext(&runtime_key(), CredentialKind::WebhookSecret)
            .await
            .unwrap()
            .as_str(),
        "super-secret"
    );

    let outcome = handler
        .revoke(RevokeStoredCredential::new(credential_ref.clone()))
        .await
        .unwrap();

    assert_eq!(outcome.stream_position(), position(3));
    assert!(matches!(
        resolver.resolve(&runtime_key(), CredentialKind::WebhookSecret).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));

    runtime_credentials
        .projections()
        .upsert(RuntimeIntegrationProjection::active_from_credential_ref(credential_ref, 3).unwrap())
        .await;
    assert!(matches!(
        resolver.resolve(&runtime_key(), CredentialKind::WebhookSecret).await,
        Err(RuntimeCredentialError::SecretStore(SecretStoreError::Unreadable { .. }))
    ));
}

#[tokio::test]
async fn runtime_handler_destroy_removes_projection_and_invalidates_cached_secret() {
    let events = HandlerTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let runtime_credentials = RuntimeCredentialRegistry::default();
    let resolver = runtime_credentials.resolver(secrets.clone());
    let handler = CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials.clone());
    let outcome = handler.put(put_command("super-secret")).await.unwrap();
    let state = outcome.into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let credential_ref = active_metadata.reference().clone();
    handler
        .revoke(RevokeStoredCredential::new(credential_ref.clone()))
        .await
        .unwrap();

    let outcome = handler
        .destroy(DestroyStoredCredential::new(credential_ref.clone(), destroy_reason()))
        .await
        .unwrap();

    assert_eq!(outcome.stream_position(), position(5));
    assert!(matches!(
        resolver.resolve(&runtime_key(), CredentialKind::WebhookSecret).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));

    runtime_credentials
        .projections()
        .upsert(RuntimeIntegrationProjection::active_from_credential_ref(credential_ref, 5).unwrap())
        .await;
    assert!(matches!(
        resolver.resolve(&runtime_key(), CredentialKind::WebhookSecret).await,
        Err(RuntimeCredentialError::SecretStore(SecretStoreError::Unreadable { .. }))
    ));
}

#[tokio::test]
async fn runtime_handler_with_openbao_testcontainer_applies_state_and_resolves_precise_value() {
    let server = OpenBaoServer::start().await;
    let secrets = server.store();
    let events = HandlerTestStreamStore::default();
    let runtime_credentials = RuntimeCredentialRegistry::default();
    let resolver = runtime_credentials.resolver(secrets.clone());
    let handler = CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials.clone());

    let put = handler.put(put_command("copy-this-value-in-and-out")).await.unwrap();

    assert_eq!(put.stream_position(), position(2));
    let put_state = put.into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = put_state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (active_metadata, _) = decode_active_state("active", active).unwrap();
    let initial_ref = active_metadata.reference().clone();
    assert_eq!(
        resolver
            .resolve_plaintext(&runtime_key(), CredentialKind::WebhookSecret)
            .await
            .unwrap()
            .as_str(),
        "copy-this-value-in-and-out"
    );

    let rotated = handler
        .rotate(RotateCredential::new(
            initial_ref.clone(),
            SecretString::new("rotated-copy-this-value-in-and-out").unwrap(),
        ))
        .await
        .unwrap();

    assert_eq!(rotated.stream_position(), position(4));
    let rotated_state = rotated.into_state();
    let Some(CredentialStateSnapshotCase::Active(active)) = rotated_state.state.as_ref() else {
        panic!("expected active credential");
    };
    let (rotated_metadata, _) = decode_active_state("active", active).unwrap();
    let rotated_ref = rotated_metadata.reference().clone();
    assert_eq!(rotated_ref.version().get(), 2);
    assert_eq!(
        resolver
            .resolve_plaintext(&runtime_key(), CredentialKind::WebhookSecret)
            .await
            .unwrap()
            .as_str(),
        "rotated-copy-this-value-in-and-out"
    );

    let revoked = handler
        .revoke(RevokeStoredCredential::new(rotated_ref.clone()))
        .await
        .unwrap();

    assert_eq!(revoked.stream_position(), position(5));
    assert!(matches!(
        revoked.state().state.as_ref(),
        Some(CredentialStateSnapshotCase::Revoked(_))
    ));
    assert!(matches!(
        resolver.resolve(&runtime_key(), CredentialKind::WebhookSecret).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));
    assert!(matches!(
        secrets.get(&rotated_ref).await,
        Err(SecretStoreError::Unreadable { .. })
    ));

    assert_eq!(events.decoded_events().len(), 5);
    for event in events.events() {
        assert!(!payload_contains(&event.event.content, "copy-this-value-in-and-out"));
        assert!(!payload_contains(
            &event.event.content,
            "rotated-copy-this-value-in-and-out"
        ));
    }
}
