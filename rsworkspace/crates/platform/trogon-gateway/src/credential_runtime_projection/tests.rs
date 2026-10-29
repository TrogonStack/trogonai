use std::sync::{Arc, Mutex};

use crate::credential::commands::domain::{
    AllowedHosts, AllowedRuntimeServices, CredentialDestroyReason, CredentialFailureReason, CredentialMetadata,
    CredentialScope, CredentialVersion, InjectionLocation, InjectionLocations, RuntimeServiceId,
};
use crate::credential::commands::proto::{
    activated_to_proto, decode_scope_key, destroy_failed_to_proto, destroy_requested_to_proto, destroyed_to_proto,
    revoked_to_proto, write_requested_to_proto,
};
use crate::secret_store::{
    MockOpenBaoSecretStore, SecretDestroyReason, SecretStoreDestroy, SecretStoreMetadata, SecretStorePut,
    SecretStoreRevoke, SecretStoreRotate, SecretVerifier,
};
use chrono::Utc;
use trogon_decider_nats::{StreamSubject, append_stream};
use trogon_decider_runtime::{
    Event, EventEncode, EventId, EventType, Headers, ReadFrom, ReadStreamRequest, ReadStreamResponse, StreamEvent,
    StreamPosition, StreamRead,
};
use trogon_nats::jetstream::{MockJetStreamKvStore, MockJetStreamPublishMessage};
use uuid::Uuid;

use super::*;

#[derive(Clone, Default)]
struct ProjectionTestEventStore {
    events: Arc<Mutex<Vec<StreamEvent>>>,
}

#[derive(Debug, thiserror::Error)]
#[error("projection test event store failed")]
struct ProjectionTestEventStoreError;

impl ProjectionTestEventStore {
    fn push(&self, stream_id: &str, stream_position: u64, event: v1::CredentialEvent) {
        self.events
            .lock()
            .unwrap()
            .push(stream_event(stream_id, stream_position, event));
    }
}

impl StreamRead<str> for ProjectionTestEventStore {
    type Error = ProjectionTestEventStoreError;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        let start = match request.from {
            ReadFrom::Beginning => 1,
            ReadFrom::Position(position) => position.as_u64(),
        };
        let events = self.events.lock().unwrap();
        let current_position = events
            .iter()
            .filter(|event| event.stream_id() == request.stream_id)
            .map(|event| event.stream_position)
            .max();

        Ok(ReadStreamResponse {
            current_position,
            events: events
                .iter()
                .filter(|event| event.stream_id() == request.stream_id && event.stream_position.as_u64() >= start)
                .cloned()
                .collect(),
        })
    }
}

fn integration_id() -> SourceIntegrationId {
    SourceIntegrationId::new("primary").unwrap()
}

fn owner_id() -> CredentialOwnerId {
    CredentialOwnerId::new("tenant-1").unwrap()
}

fn key() -> RuntimeIntegrationKey {
    RuntimeIntegrationKey::new(SourceKind::Discord, &integration_id())
}

fn source_key() -> RuntimeIntegrationKey {
    RuntimeIntegrationKey::for_source(SourceKind::Discord)
}

async fn put_bot_token(store: &MockOpenBaoSecretStore, value: &str) -> CredentialRef {
    store
        .put(
            CredentialScope::integration(owner_id(), SourceKind::Discord, integration_id()),
            CredentialKind::BotToken,
            SecretString::new(value).unwrap(),
        )
        .await
        .unwrap()
}

async fn put_source_bot_token(store: &MockOpenBaoSecretStore, value: &str) -> CredentialRef {
    store
        .put(
            CredentialScope::source(owner_id(), SourceKind::Discord),
            CredentialKind::BotToken,
            SecretString::new(value).unwrap(),
        )
        .await
        .unwrap()
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

fn runtime_event(id: u64, event: v1::CredentialEvent) -> Event {
    Event {
        id: EventId::new(Uuid::from_u128(id as u128)),
        r#type: EventType::event_type(&event).unwrap().to_string(),
        content: EventEncode::encode(&event).unwrap(),
        headers: Headers::empty(),
    }
}

async fn raw_stream(events: impl IntoIterator<Item = Event>) -> MockJetStreamPublishMessage {
    let stream = MockJetStreamPublishMessage::new();
    let events = events.into_iter().collect::<Vec<_>>();
    if events.is_empty() {
        return stream;
    }
    append_stream(
        &stream,
        StreamSubject::new("gateway.credentials.events.v1.raw").unwrap(),
        None,
        &events,
    )
    .await
    .unwrap();
    stream
}

fn write_requested(credential: &CredentialRef) -> v1::CredentialEvent {
    let scope = decode_scope_key(
        credential.owner_id().clone(),
        credential.source(),
        "scope_key",
        credential.scope_key(),
    )
    .unwrap();
    v1::CredentialEvent {
        event: Some(write_requested_to_proto(credential.id(), &scope, credential.kind()).into()),
    }
}

fn activated(metadata: &CredentialMetadata) -> v1::CredentialEvent {
    v1::CredentialEvent {
        event: Some(activated_to_proto(metadata).into()),
    }
}

fn revoked(credential: &CredentialRef) -> v1::CredentialEvent {
    v1::CredentialEvent {
        event: Some(revoked_to_proto(credential).into()),
    }
}

fn destroy_requested(credential: &CredentialRef) -> v1::CredentialEvent {
    let reason = CredentialDestroyReason::new("credential lifecycle cleanup").unwrap();
    v1::CredentialEvent {
        event: Some(destroy_requested_to_proto(credential, &reason).into()),
    }
}

fn destroy_failed(credential: &CredentialRef) -> v1::CredentialEvent {
    let reason = CredentialFailureReason::new("openbao destroy failed").unwrap();
    v1::CredentialEvent {
        event: Some(destroy_failed_to_proto(credential, &reason).into()),
    }
}

fn build_state(events: impl IntoIterator<Item = v1::CredentialEvent>) -> state_v1::CredentialStateSnapshot {
    events
        .into_iter()
        .try_fold(initial_state(), |state, event| evolve(state, &event))
        .unwrap()
}

#[tokio::test]
async fn active_projection_can_be_built_from_credential_ref() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let projection = RuntimeIntegrationProjection::active_from_credential_ref(credential, 7).unwrap();
    let projections = InMemoryRuntimeProjectionRepository::default();
    projections.upsert(projection.clone()).await;
    let resolver = RuntimeCredentialResolver::new(projections, store);

    assert_eq!(projection.key(), &key());
    assert_eq!(projection.owner_id(), &owner_id());
    assert_eq!(projection.status(), RuntimeIntegrationStatus::Active);
    assert_eq!(projection.version(), 7);
    assert_eq!(
        resolver
            .resolve_plaintext(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_str(),
        "Bot token"
    );
}

#[tokio::test]
async fn active_projection_can_be_refreshed_from_replayed_state() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let state = [
        write_requested(&credential),
        v1::CredentialEvent {
            event: Some(activated_to_proto(&metadata).into()),
        },
    ]
    .into_iter()
    .try_fold(initial_state(), |state, event| evolve(state, &event))
    .unwrap();
    let projection = RuntimeIntegrationProjection::from_credential_state(&state, 2)
        .unwrap()
        .unwrap();
    let projections = InMemoryRuntimeProjectionRepository::default();
    projections.upsert(projection).await;
    let resolver = RuntimeCredentialResolver::new(projections, store);

    assert_eq!(
        resolver
            .resolve_plaintext(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_str(),
        "Bot token"
    );
}

#[tokio::test]
async fn refresh_rebuilds_runtime_projection_from_credential_events() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store);

    let report = registry
        .refresh_from_credential_events([
            stream_event("credential-a", 1, write_requested(&credential)),
            stream_event(
                "credential-a",
                2,
                v1::CredentialEvent {
                    event: Some(activated_to_proto(&metadata).into()),
                },
            ),
        ])
        .await
        .unwrap();

    assert_eq!(report.scanned_events(), 2);
    assert_eq!(report.decoded_events(), 2);
    assert_eq!(report.skipped_events(), 0);
    assert_eq!(report.projected_integrations(), 1);
    assert_eq!(
        resolver
            .resolve_plaintext(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_str(),
        "Bot token"
    );
}

#[tokio::test]
async fn refresh_rebuilds_runtime_projection_from_persisted_credential_stream() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let stream = MockJetStreamPublishMessage::new();
    append_stream(
        &stream,
        StreamSubject::new("credential-a").unwrap(),
        None,
        &[
            runtime_event(1, write_requested(&credential)),
            runtime_event(
                2,
                v1::CredentialEvent {
                    event: Some(activated_to_proto(&metadata).into()),
                },
            ),
        ],
    )
    .await
    .unwrap();
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store);

    let report = registry.refresh_from_credential_stream(&stream, 1).await.unwrap();

    assert_eq!(report.scanned_events(), 2);
    assert_eq!(report.decoded_events(), 2);
    assert_eq!(report.projected_integrations(), 1);
    assert_eq!(
        resolver
            .resolve_plaintext(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_str(),
        "Bot token"
    );
}

#[tokio::test]
async fn projection_checkpoint_store_loads_default_when_missing() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    let checkpoints = RuntimeProjectionKvCheckpointStore::new(kv.clone());

    let checkpoint = checkpoints.load().await.unwrap();

    assert_eq!(checkpoint.last_scanned_sequence(), 0);
    assert_eq!(kv.entry_calls(), vec![RUNTIME_PROJECTION_CHECKPOINT_KEY.to_string()]);
    assert!(kv.create_calls().is_empty());
    assert!(kv.update_calls().is_empty());
}

#[tokio::test]
async fn projection_checkpoint_store_saves_cursor() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    let checkpoints = RuntimeProjectionKvCheckpointStore::new(kv.clone());

    checkpoints.save(RuntimeProjectionCheckpoint::new(42)).await.unwrap();

    let calls = kv.create_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, RUNTIME_PROJECTION_CHECKPOINT_KEY);
    assert_eq!(
        decode_projection_checkpoint(&calls[0].1)
            .unwrap()
            .last_scanned_sequence(),
        42
    );
}

#[tokio::test]
async fn checkpointed_refresh_advances_projection_checkpoint_after_success() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let event_store = ProjectionTestEventStore::default();
    event_store.push(credential.id().as_str(), 1, write_requested(&credential));
    event_store.push(
        credential.id().as_str(),
        2,
        v1::CredentialEvent {
            event: Some(activated_to_proto(&metadata).into()),
        },
    );
    let stream = raw_stream([
        runtime_event(1, write_requested(&credential)),
        runtime_event(
            2,
            v1::CredentialEvent {
                event: Some(activated_to_proto(&metadata).into()),
            },
        ),
    ])
    .await;
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    kv.enqueue_entry_none();
    let checkpoints = RuntimeProjectionKvCheckpointStore::new(kv.clone());
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store);

    let report = registry
        .refresh_from_credential_stream_checkpointed(&stream, &event_store, &checkpoints)
        .await
        .unwrap();

    assert_eq!(report.scanned_events(), 2);
    assert_eq!(report.decoded_events(), 2);
    assert_eq!(report.changed_credentials(), 1);
    assert_eq!(report.applied_credentials(), 1);
    assert_eq!(report.projected_integrations(), 1);
    assert_eq!(report.checkpoint_loaded_sequence(), 0);
    assert_eq!(report.checkpoint_advanced_to(), Some(2));
    assert_eq!(
        resolver
            .resolve_plaintext(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_str(),
        "Bot token"
    );
    let calls = kv.create_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        decode_projection_checkpoint(&calls[0].1)
            .unwrap()
            .last_scanned_sequence(),
        2
    );
}

#[tokio::test]
async fn checkpointed_refresh_starts_after_loaded_checkpoint() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let event_store = ProjectionTestEventStore::default();
    event_store.push(credential.id().as_str(), 1, write_requested(&credential));
    event_store.push(
        credential.id().as_str(),
        2,
        v1::CredentialEvent {
            event: Some(activated_to_proto(&metadata).into()),
        },
    );
    let stream = raw_stream([
        runtime_event(1, write_requested(&credential)),
        runtime_event(
            2,
            v1::CredentialEvent {
                event: Some(activated_to_proto(&metadata).into()),
            },
        ),
    ])
    .await;
    let checkpoint = RuntimeProjectionCheckpoint::new(1);
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(
        Bytes::from(encode_projection_checkpoint(checkpoint)),
        3,
        kv::Operation::Put,
    );
    kv.enqueue_entry(
        Bytes::from(encode_projection_checkpoint(checkpoint)),
        4,
        kv::Operation::Put,
    );
    let checkpoints = RuntimeProjectionKvCheckpointStore::new(kv.clone());
    let registry = RuntimeCredentialRegistry::default();

    let report = registry
        .refresh_from_credential_stream_checkpointed(&stream, &event_store, &checkpoints)
        .await
        .unwrap();

    assert_eq!(report.scanned_events(), 1);
    assert_eq!(report.decoded_events(), 1);
    assert_eq!(report.changed_credentials(), 1);
    assert_eq!(report.applied_credentials(), 1);
    assert_eq!(report.checkpoint_loaded_sequence(), 1);
    assert_eq!(report.checkpoint_advanced_to(), Some(2));
    let calls = kv.update_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        decode_projection_checkpoint(&calls[0].1)
            .unwrap()
            .last_scanned_sequence(),
        2
    );
    assert_eq!(calls[0].2, 4);
}

#[tokio::test]
async fn checkpointed_refresh_does_not_save_checkpoint_when_no_new_events() {
    let event_store = ProjectionTestEventStore::default();
    let stream = raw_stream(Vec::new()).await;
    let checkpoint = RuntimeProjectionCheckpoint::new(2);
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(
        Bytes::from(encode_projection_checkpoint(checkpoint)),
        3,
        kv::Operation::Put,
    );
    let checkpoints = RuntimeProjectionKvCheckpointStore::new(kv.clone());
    let registry = RuntimeCredentialRegistry::default();

    let report = registry
        .refresh_from_credential_stream_checkpointed(&stream, &event_store, &checkpoints)
        .await
        .unwrap();

    assert_eq!(report.scanned_events(), 0);
    assert_eq!(report.checkpoint_loaded_sequence(), 2);
    assert_eq!(report.checkpoint_advanced_to(), None);
    assert!(kv.create_calls().is_empty());
    assert!(kv.update_calls().is_empty());
}

#[tokio::test]
async fn refresh_removes_revoked_projection_and_clears_stale_cache() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store.clone());

    registry
        .refresh_from_credential_events([
            stream_event("credential-a", 1, write_requested(&credential)),
            stream_event(
                "credential-a",
                2,
                v1::CredentialEvent {
                    event: Some(activated_to_proto(&metadata).into()),
                },
            ),
        ])
        .await
        .unwrap();
    assert_eq!(
        resolver
            .resolve_plaintext(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_str(),
        "Bot token"
    );

    store.revoke(&credential).await.unwrap();
    let report = registry
        .refresh_from_credential_events([
            stream_event("credential-a", 1, write_requested(&credential)),
            stream_event(
                "credential-a",
                2,
                v1::CredentialEvent {
                    event: Some(activated_to_proto(&metadata).into()),
                },
            ),
            stream_event(
                "credential-a",
                3,
                v1::CredentialEvent {
                    event: Some(revoked_to_proto(&credential).into()),
                },
            ),
        ])
        .await
        .unwrap();

    assert_eq!(report.projected_integrations(), 0);
    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));

    registry
        .projections()
        .upsert(projection(RuntimeIntegrationStatus::Active, credential))
        .await;
    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::SecretStore(SecretStoreError::Unreadable { .. }))
    ));
}

#[tokio::test]
async fn refresh_projects_source_scoped_active_state() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_source_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store);

    let report = registry
        .refresh_from_credential_events([
            stream_event("credential-a", 1, write_requested(&credential)),
            stream_event(
                "credential-a",
                2,
                v1::CredentialEvent {
                    event: Some(activated_to_proto(&metadata).into()),
                },
            ),
        ])
        .await
        .unwrap();

    assert_eq!(report.scanned_events(), 2);
    assert_eq!(report.decoded_events(), 2);
    assert_eq!(report.projected_integrations(), 1);
    assert_eq!(
        resolver
            .resolve_plaintext(&source_key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_str(),
        "Bot token"
    );
}

#[tokio::test]
async fn registry_applies_active_state_incrementally() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store);
    let state = build_state([
        write_requested(&credential),
        v1::CredentialEvent {
            event: Some(activated_to_proto(&metadata).into()),
        },
    ]);

    registry.apply_state(&state, position(2)).await.unwrap();

    assert_eq!(
        resolver
            .resolve_plaintext(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_str(),
        "Bot token"
    );
}

#[tokio::test]
async fn registry_apply_projects_source_scoped_active_state() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_source_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store);
    let state = build_state([
        write_requested(&credential),
        v1::CredentialEvent {
            event: Some(activated_to_proto(&metadata).into()),
        },
    ]);

    registry.apply_state(&state, position(2)).await.unwrap();

    assert_eq!(
        resolver
            .resolve_plaintext(&source_key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_str(),
        "Bot token"
    );
}

#[tokio::test]
async fn registry_applies_revoked_state_incrementally_and_invalidates_cache() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store.clone());
    let active_state = build_state([
        write_requested(&credential),
        v1::CredentialEvent {
            event: Some(activated_to_proto(&metadata).into()),
        },
    ]);
    registry.apply_state(&active_state, position(2)).await.unwrap();
    assert_eq!(
        resolver
            .resolve_plaintext(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_str(),
        "Bot token"
    );

    store.revoke(&credential).await.unwrap();
    let revoked_state = evolve(
        active_state,
        &v1::CredentialEvent {
            event: Some(revoked_to_proto(&credential).into()),
        },
    )
    .unwrap();
    registry.apply_state(&revoked_state, position(3)).await.unwrap();

    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));

    registry
        .projections()
        .upsert(projection(RuntimeIntegrationStatus::Active, credential))
        .await;
    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::SecretStore(SecretStoreError::Unreadable { .. }))
    ));
}

#[tokio::test]
async fn registry_applies_destroyed_state_incrementally_and_invalidates_cache() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store.clone());
    let active_state = build_state([
        write_requested(&credential),
        v1::CredentialEvent {
            event: Some(activated_to_proto(&metadata).into()),
        },
    ]);
    registry.apply_state(&active_state, position(2)).await.unwrap();
    assert_eq!(
        resolver
            .resolve_plaintext(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_str(),
        "Bot token"
    );

    store.revoke(&credential).await.unwrap();
    let revoked_state = evolve(
        active_state,
        &v1::CredentialEvent {
            event: Some(revoked_to_proto(&credential).into()),
        },
    )
    .unwrap();
    registry.apply_state(&revoked_state, position(3)).await.unwrap();

    let reason = SecretDestroyReason::new("credential lifecycle cleanup").unwrap();
    let destroy_reason = CredentialDestroyReason::new(reason.as_str()).unwrap();
    let destroy_requested_state = evolve(
        revoked_state,
        &v1::CredentialEvent {
            event: Some(destroy_requested_to_proto(&credential, &destroy_reason).into()),
        },
    )
    .unwrap();
    registry
        .apply_state(&destroy_requested_state, position(4))
        .await
        .unwrap();

    store.destroy(&credential, &reason).await.unwrap();
    let destroyed_state = evolve(
        destroy_requested_state,
        &v1::CredentialEvent {
            event: Some(destroyed_to_proto(&credential).into()),
        },
    )
    .unwrap();
    registry.apply_state(&destroyed_state, position(5)).await.unwrap();

    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));

    registry
        .projections()
        .upsert(projection(RuntimeIntegrationStatus::Active, credential))
        .await;
    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::SecretStore(SecretStoreError::Unreadable { .. }))
    ));
}

#[tokio::test]
async fn registry_applies_destroy_requested_state_incrementally_and_invalidates_cache() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store.clone());
    let active_state = build_state([write_requested(&credential), activated(&metadata)]);
    registry.apply_state(&active_state, position(2)).await.unwrap();
    assert_eq!(
        resolver
            .resolve_plaintext(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_str(),
        "Bot token"
    );

    store.revoke(&credential).await.unwrap();
    let destroy_requested_state = build_state([
        write_requested(&credential),
        activated(&metadata),
        revoked(&credential),
        destroy_requested(&credential),
    ]);
    registry
        .apply_state(&destroy_requested_state, position(4))
        .await
        .unwrap();

    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));

    registry
        .projections()
        .upsert(projection(RuntimeIntegrationStatus::Active, credential))
        .await;
    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::SecretStore(SecretStoreError::Unreadable { .. }))
    ));
}

#[tokio::test]
async fn registry_applies_cleanup_failed_state_incrementally_and_invalidates_cache() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store.clone());
    let active_state = build_state([write_requested(&credential), activated(&metadata)]);
    registry.apply_state(&active_state, position(2)).await.unwrap();
    assert_eq!(
        resolver
            .resolve_plaintext(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_str(),
        "Bot token"
    );

    store.revoke(&credential).await.unwrap();
    let cleanup_failed_state = build_state([
        write_requested(&credential),
        activated(&metadata),
        revoked(&credential),
        destroy_requested(&credential),
        destroy_failed(&credential),
    ]);
    registry.apply_state(&cleanup_failed_state, position(5)).await.unwrap();

    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));

    registry
        .projections()
        .upsert(projection(RuntimeIntegrationStatus::Active, credential))
        .await;
    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::SecretStore(SecretStoreError::Unreadable { .. }))
    ));
}

#[test]
fn source_scoped_credential_ref_builds_source_runtime_projection_key() {
    let scope = CredentialScope::source(owner_id(), SourceKind::Discord);
    let credential = CredentialRef::new(
        CredentialId::new("openbao:tenant-1:discord:bot_token").unwrap(),
        CredentialVersion::initial(),
        &scope,
        CredentialKind::BotToken,
    );
    let key = RuntimeIntegrationKey::from_credential_ref(&credential).unwrap();

    assert_eq!(key, source_key());
    assert_eq!(key.integration_id(), None);
}

fn projection(status: RuntimeIntegrationStatus, credential: CredentialRef) -> RuntimeIntegrationProjection {
    RuntimeIntegrationProjection::new(owner_id(), SourceKind::Discord, integration_id(), status, 1)
        .with_credential(CredentialKind::BotToken, credential)
}

/// Collects everything the tracing layer writes so a test can assert what
/// never reached it.
#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl CapturedLogs {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

use tracing_subscriber::util::SubscriberInitExt;

const LEAK_CANARY: &str = "canary-plaintext-must-never-be-logged";

#[tokio::test]
async fn resolving_and_rotating_never_writes_plaintext_to_the_tracing_layer() {
    let logs = CapturedLogs::default();
    let _guard = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(logs.clone())
        .set_default();

    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, LEAK_CANARY).await;
    let projections = InMemoryRuntimeProjectionRepository::default();
    projections
        .upsert(projection(RuntimeIntegrationStatus::Active, credential.clone()))
        .await;
    let resolver = RuntimeCredentialResolver::new(projections.clone(), store.clone());

    let material = resolver.resolve(&key(), CredentialKind::BotToken).await.unwrap();
    assert_eq!(material.as_plaintext().unwrap().as_str(), LEAK_CANARY);

    let rotated = store
        .rotate(&credential, SecretString::new(LEAK_CANARY).unwrap())
        .await
        .unwrap();
    projections
        .upsert(projection(RuntimeIntegrationStatus::Active, rotated.clone()))
        .await;
    resolver.cache().invalidate(&credential).await;
    resolver.resolve(&key(), CredentialKind::BotToken).await.unwrap();

    store.revoke(&rotated).await.unwrap();
    resolver.cache().invalidate(&rotated).await;
    let revoked = resolver.resolve(&key(), CredentialKind::BotToken).await.unwrap_err();

    tracing::error!(error = ?revoked, "captured resolve failure");
    tracing::error!(error = %revoked, "captured resolve failure");
    tracing::info!(material = ?material, credential = %rotated, "captured resolved material");
    tracing::info!(projection = ?projections.get(&key()).await, "captured projection");

    let contents = logs.contents();
    assert!(contents.contains("captured resolve failure"));
    assert!(contents.contains("captured resolved material"));
    assert!(
        !contents.contains(LEAK_CANARY),
        "plaintext leaked into logs: {contents}"
    );
}

#[test]
fn every_carrier_of_plaintext_redacts_its_debug_rendering() {
    let material = SecretMaterial::Plaintext(SecretString::new(LEAK_CANARY).unwrap());
    let verifier = SecretMaterial::Verifier(SecretVerifier::new(LEAK_CANARY).unwrap());

    assert!(!format!("{material:?}").contains(LEAK_CANARY));
    assert!(!format!("{verifier:?}").contains(LEAK_CANARY));
}

#[tokio::test]
async fn a_delivery_denial_message_carries_no_plaintext() {
    let (resolver, _) = restricted_resolver(
        RuntimeReleasePolicy::default().with_allowed_hosts(AllowedHosts::only(["api.example.com"]).unwrap()),
    )
    .await;

    let denied = resolver
        .resolve_for(
            &key(),
            CredentialKind::BotToken,
            &RuntimeReleaseRequest::new().to_host("api.evil.net"),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        &denied,
        RuntimeCredentialError::DeliveryDenied {
            denied: RuntimeReleaseDeniedError::Host { host: Some(host) },
            ..
        } if host == "api.evil.net"
    ));
    assert!(!format!("{denied:?}").contains("Bot token"));
}

#[tokio::test]
async fn resolves_active_projection_from_secret_store() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let projections = InMemoryRuntimeProjectionRepository::default();
    projections
        .upsert(projection(RuntimeIntegrationStatus::Active, credential))
        .await;
    let resolver = RuntimeCredentialResolver::new(projections, store);

    let token = resolver
        .resolve_plaintext(&key(), CredentialKind::BotToken)
        .await
        .unwrap();

    assert_eq!(token.as_str(), "Bot token");
}

#[tokio::test]
async fn disabled_projection_fails_closed_without_reading_secret_store() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let projections = InMemoryRuntimeProjectionRepository::default();
    projections
        .upsert(projection(RuntimeIntegrationStatus::Disabled, credential))
        .await;
    let resolver = RuntimeCredentialResolver::new(projections, store);

    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::IntegrationNotResolvable {
            status: RuntimeIntegrationStatus::Disabled,
            ..
        })
    ));
}

async fn restricted_resolver(
    policy: RuntimeReleasePolicy,
) -> (RuntimeCredentialResolver<MockOpenBaoSecretStore>, CredentialRef) {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let projections = InMemoryRuntimeProjectionRepository::default();
    projections
        .upsert(projection(RuntimeIntegrationStatus::Active, credential.clone()).with_release_policy(policy))
        .await;
    (RuntimeCredentialResolver::new(projections, store), credential)
}

#[test]
fn a_wildcard_host_covers_one_label_and_not_the_apex() {
    let hosts = AllowedHosts::only(["*.example.com", "trogonai.dev"]).unwrap();

    assert!(hosts.permits(Some("api.example.com")));
    assert!(hosts.permits(Some("API.Example.COM.")));
    assert!(!hosts.permits(Some("example.com")));
    assert!(!hosts.permits(Some("a.b.example.com")));
    assert!(hosts.permits(Some("trogonai.dev")));
    assert!(!hosts.permits(Some("api.trogonai.dev")));
    assert!(!hosts.permits(None));
}

#[tokio::test]
async fn unconfigured_release_policy_keeps_shipped_source_paths_resolving() {
    let (resolver, _) = restricted_resolver(RuntimeReleasePolicy::default()).await;

    assert_eq!(
        resolver
            .resolve_plaintext(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_str(),
        "Bot token"
    );
}

#[tokio::test]
async fn host_restriction_denies_an_unlisted_host_and_permits_a_listed_one() {
    let (resolver, _) = restricted_resolver(
        RuntimeReleasePolicy::default().with_allowed_hosts(AllowedHosts::only(["*.example.com"]).unwrap()),
    )
    .await;

    let denied = resolver
        .resolve_for(
            &key(),
            CredentialKind::BotToken,
            &RuntimeReleaseRequest::new().to_host("api.evil.net"),
        )
        .await
        .unwrap_err();
    assert!(denied.is_delivery_denied());

    assert!(
        resolver
            .resolve_for(
                &key(),
                CredentialKind::BotToken,
                &RuntimeReleaseRequest::new().to_host("api.example.com:443"),
            )
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn host_restriction_denies_the_unqualified_resolve_path() {
    let (resolver, _) = restricted_resolver(
        RuntimeReleasePolicy::default().with_allowed_hosts(AllowedHosts::only(["api.example.com"]).unwrap()),
    )
    .await;

    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::DeliveryDenied {
            denied: RuntimeReleaseDeniedError::Host { .. },
            ..
        })
    ));
}

#[tokio::test]
async fn unauthorized_runtime_service_cannot_resolve() {
    let (resolver, _) = restricted_resolver(
        RuntimeReleasePolicy::default()
            .with_allowed_runtime_services(AllowedRuntimeServices::only(["trogon-gateway"]).unwrap()),
    )
    .await;
    let intruder = RuntimeServiceId::new("some-other-worker").unwrap();
    let allowed = RuntimeServiceId::new("trogon-gateway").unwrap();

    assert!(matches!(
        resolver
            .resolve_for(
                &key(),
                CredentialKind::BotToken,
                &RuntimeReleaseRequest::new().by_runtime_service(&intruder),
            )
            .await,
        Err(RuntimeCredentialError::DeliveryDenied {
            denied: RuntimeReleaseDeniedError::RuntimeService { .. },
            ..
        })
    ));
    assert!(
        resolver
            .resolve_for(
                &key(),
                CredentialKind::BotToken,
                &RuntimeReleaseRequest::new().by_runtime_service(&allowed),
            )
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn injection_location_restriction_denies_a_query_parameter_placement() {
    let header = InjectionLocation::header("authorization").unwrap();
    let query = InjectionLocation::query_parameter("access_token").unwrap();
    let (resolver, _) = restricted_resolver(
        RuntimeReleasePolicy::default().with_injection_locations(InjectionLocations::new([header.clone()])),
    )
    .await;

    assert!(matches!(
        resolver
            .resolve_for(
                &key(),
                CredentialKind::BotToken,
                &RuntimeReleaseRequest::new().at_injection_location(&query),
            )
            .await,
        Err(RuntimeCredentialError::DeliveryDenied {
            denied: RuntimeReleaseDeniedError::InjectionLocation { .. },
            ..
        })
    ));
    assert!(
        resolver
            .resolve_for(
                &key(),
                CredentialKind::BotToken,
                &RuntimeReleaseRequest::new().at_injection_location(&header),
            )
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn a_denied_request_does_not_warm_the_cache() {
    let (resolver, credential) = restricted_resolver(
        RuntimeReleasePolicy::default().with_allowed_hosts(AllowedHosts::only(["api.example.com"]).unwrap()),
    )
    .await;

    let _ = resolver
        .resolve_for(
            &key(),
            CredentialKind::BotToken,
            &RuntimeReleaseRequest::new().to_host("api.evil.net"),
        )
        .await;

    assert!(resolver.cache().expires_at(&credential).await.is_none());
}

#[tokio::test]
async fn a_denial_does_not_reveal_whether_the_credential_exists() {
    let policy = RuntimeReleasePolicy::default().with_allowed_hosts(AllowedHosts::only(["api.example.com"]).unwrap());
    let store = MockOpenBaoSecretStore::default();
    let projections = InMemoryRuntimeProjectionRepository::default();
    projections
        .upsert(
            RuntimeIntegrationProjection::new(
                owner_id(),
                SourceKind::Discord,
                integration_id(),
                RuntimeIntegrationStatus::Active,
                1,
            )
            .with_release_policy(policy),
        )
        .await;
    let resolver = RuntimeCredentialResolver::new(projections, store);

    assert!(matches!(
        resolver
            .resolve_for(
                &key(),
                CredentialKind::BotToken,
                &RuntimeReleaseRequest::new().to_host("api.evil.net"),
            )
            .await,
        Err(RuntimeCredentialError::DeliveryDenied { .. })
    ));
}

#[tokio::test]
async fn cache_ttl_override_shortens_the_cached_window() {
    let (default_resolver, default_credential) = restricted_resolver(RuntimeReleasePolicy::default()).await;
    let (short_resolver, short_credential) = restricted_resolver(
        RuntimeReleasePolicy::default()
            .with_cache_ttl_override(Duration::from_secs(5))
            .unwrap(),
    )
    .await;

    default_resolver
        .resolve(&key(), CredentialKind::BotToken)
        .await
        .unwrap();
    short_resolver.resolve(&key(), CredentialKind::BotToken).await.unwrap();

    let now = Instant::now();
    let default_remaining = default_resolver
        .cache()
        .expires_at(&default_credential)
        .await
        .unwrap()
        .saturating_duration_since(now);
    let short_remaining = short_resolver
        .cache()
        .expires_at(&short_credential)
        .await
        .unwrap()
        .saturating_duration_since(now);

    assert!(short_remaining <= Duration::from_secs(5));
    assert!(short_remaining < default_remaining);
}

#[tokio::test]
async fn cache_ttl_override_may_not_extend_the_cached_window() {
    let (resolver, credential) = restricted_resolver(
        RuntimeReleasePolicy::default()
            .with_cache_ttl_override(Duration::from_secs(86_400))
            .unwrap(),
    )
    .await;

    resolver.resolve(&key(), CredentialKind::BotToken).await.unwrap();

    let remaining = resolver
        .cache()
        .expires_at(&credential)
        .await
        .unwrap()
        .saturating_duration_since(Instant::now());
    assert!(remaining <= RuntimeCredentialCachePolicy::default().ttl());
}

#[tokio::test]
async fn rotation_uses_new_credential_ref_without_reusing_stale_cache() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot old-token").await;
    let projections = InMemoryRuntimeProjectionRepository::default();
    projections
        .upsert(projection(RuntimeIntegrationStatus::Active, credential.clone()))
        .await;
    let resolver = RuntimeCredentialResolver::new(projections.clone(), store.clone());

    assert_eq!(
        resolver
            .resolve(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_plaintext()
            .unwrap()
            .as_str(),
        "Bot old-token"
    );

    let rotated = store
        .rotate(&credential, SecretString::new("Bot new-token").unwrap())
        .await
        .unwrap();
    projections
        .upsert(projection(RuntimeIntegrationStatus::Active, rotated))
        .await;

    assert_eq!(
        resolver
            .resolve(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_plaintext()
            .unwrap()
            .as_str(),
        "Bot new-token"
    );
}

#[tokio::test]
async fn revoked_store_entry_fails_after_cache_invalidation() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let projections = InMemoryRuntimeProjectionRepository::default();
    projections
        .upsert(projection(RuntimeIntegrationStatus::Active, credential.clone()))
        .await;
    let resolver = RuntimeCredentialResolver::new(projections, store.clone());

    assert!(
        resolver
            .resolve(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_plaintext()
            .is_some()
    );

    store.revoke(&credential).await.unwrap();
    resolver.cache().invalidate(&credential).await;

    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::SecretStore(SecretStoreError::Unreadable { .. }))
    ));
}

#[tokio::test]
async fn missing_required_credential_fails_closed() {
    let store = MockOpenBaoSecretStore::default();
    let projections = InMemoryRuntimeProjectionRepository::default();
    projections
        .upsert(RuntimeIntegrationProjection::new(
            owner_id(),
            SourceKind::Discord,
            integration_id(),
            RuntimeIntegrationStatus::Active,
            1,
        ))
        .await;
    let resolver = RuntimeCredentialResolver::new(projections, store);

    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::CredentialMissing {
            kind: CredentialKind::BotToken,
            ..
        })
    ));
}

fn cache_credential(id: &str) -> CredentialRef {
    CredentialRef::new(
        CredentialId::new(id).unwrap(),
        CredentialVersion::initial(),
        &CredentialScope::integration(owner_id(), SourceKind::Discord, integration_id()),
        CredentialKind::BotToken,
    )
}

fn cache_secret(value: &str) -> SecretMaterial {
    SecretMaterial::Plaintext(SecretString::new(value).unwrap())
}

#[tokio::test]
async fn cache_returns_fresh_entry_within_ttl() {
    let policy = RuntimeCredentialCachePolicy::new(Duration::from_secs(60), Duration::from_secs(5)).unwrap();
    let cache = RuntimeCredentialCache::with_policy(policy);
    let credential = cache_credential("credential-fresh");
    let now = Instant::now();

    cache.put_at(credential.clone(), cache_secret("token"), now).await;

    let material = cache.get_at(&credential, now + Duration::from_secs(1)).await.unwrap();
    assert_eq!(material.as_plaintext().unwrap().as_str(), "token");
}

#[tokio::test]
async fn cache_evicts_entry_past_its_jittered_deadline() {
    let policy = RuntimeCredentialCachePolicy::new(Duration::from_secs(60), Duration::from_secs(5)).unwrap();
    let cache = RuntimeCredentialCache::with_policy(policy);
    let credential = cache_credential("credential-expiring");
    let now = Instant::now();

    cache.put_at(credential.clone(), cache_secret("token"), now).await;
    let expires_at = cache.expires_at(&credential).await.unwrap();

    assert!(
        cache
            .get_at(&credential, expires_at + Duration::from_nanos(1))
            .await
            .is_none()
    );
    assert!(cache.expires_at(&credential).await.is_none());
}

#[tokio::test]
async fn cache_invalidate_removes_entry_explicitly() {
    let cache = RuntimeCredentialCache::default();
    let credential = cache_credential("credential-invalidate");

    cache.put(credential.clone(), cache_secret("token")).await;
    cache.invalidate(&credential).await;

    assert!(cache.get(&credential).await.is_none());
}

#[test]
fn cache_policy_rejects_zero_ttl() {
    assert!(matches!(
        RuntimeCredentialCachePolicy::new(Duration::ZERO, Duration::ZERO),
        Err(RuntimeCredentialCachePolicyError::ZeroTtl)
    ));
}

#[test]
fn cache_policy_rejects_jitter_greater_than_ttl() {
    assert!(matches!(
        RuntimeCredentialCachePolicy::new(Duration::from_secs(10), Duration::from_secs(11)),
        Err(RuntimeCredentialCachePolicyError::JitterExceedsTtl { .. })
    ));
}

#[tokio::test]
async fn cache_applies_deterministic_per_key_jitter_spread() {
    let policy = RuntimeCredentialCachePolicy::new(Duration::from_secs(60), Duration::from_secs(10)).unwrap();
    let cache = RuntimeCredentialCache::with_policy(policy);
    let credential_a = cache_credential("credential-jitter-a");
    let credential_b = cache_credential("credential-jitter-b");
    let now = Instant::now();

    cache.put_at(credential_a.clone(), cache_secret("token-a"), now).await;
    cache.put_at(credential_b.clone(), cache_secret("token-b"), now).await;

    let expires_a = cache.expires_at(&credential_a).await.unwrap();
    let expires_b = cache.expires_at(&credential_b).await.unwrap();

    assert_ne!(expires_a, expires_b);
}

fn timestamp(seconds: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(seconds, 0).unwrap()
}

#[test]
fn revocation_latency_seconds_computes_elapsed_duration() {
    let recorded_at = timestamp(1_000);
    let now = timestamp(1_005);

    assert_eq!(revocation_latency_seconds(recorded_at, now), 5.0);
}

#[test]
fn revocation_latency_seconds_clamps_negative_clock_skew_to_zero() {
    let recorded_at = timestamp(1_005);
    let now = timestamp(1_000);

    assert_eq!(revocation_latency_seconds(recorded_at, now), 0.0);
}

#[test]
fn revocation_latency_seconds_is_zero_for_identical_timestamps() {
    let at = timestamp(1_000);

    assert_eq!(revocation_latency_seconds(at, at), 0.0);
}

#[tokio::test]
async fn incremental_refresh_invalidates_cache_and_removes_projection_on_revoked_event() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let event_store = ProjectionTestEventStore::default();
    event_store.push(credential.id().as_str(), 1, write_requested(&credential));
    event_store.push(
        credential.id().as_str(),
        2,
        v1::CredentialEvent {
            event: Some(activated_to_proto(&metadata).into()),
        },
    );
    let stream = raw_stream([
        runtime_event(1, write_requested(&credential)),
        runtime_event(
            2,
            v1::CredentialEvent {
                event: Some(activated_to_proto(&metadata).into()),
            },
        ),
    ])
    .await;
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store.clone());

    registry
        .refresh_from_credential_stream_incremental(&stream, &event_store, 1)
        .await
        .unwrap();
    assert_eq!(
        resolver
            .resolve_plaintext(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_str(),
        "Bot token"
    );

    store.revoke(&credential).await.unwrap();
    event_store.push(
        credential.id().as_str(),
        3,
        v1::CredentialEvent {
            event: Some(revoked_to_proto(&credential).into()),
        },
    );
    append_stream(
        &stream,
        StreamSubject::new("gateway.credentials.events.v1.raw").unwrap(),
        None,
        &[runtime_event(
            3,
            v1::CredentialEvent {
                event: Some(revoked_to_proto(&credential).into()),
            },
        )],
    )
    .await
    .unwrap();

    let report = registry
        .refresh_from_credential_stream_incremental(&stream, &event_store, 3)
        .await
        .unwrap();

    assert_eq!(report.applied_credentials(), 1);
    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));
}

#[tokio::test]
async fn incremental_refresh_invalidates_cache_and_removes_projection_when_batch_ends_in_destroy_requested() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let event_store = ProjectionTestEventStore::default();
    event_store.push(credential.id().as_str(), 1, write_requested(&credential));
    event_store.push(credential.id().as_str(), 2, activated(&metadata));
    let stream = raw_stream([
        runtime_event(1, write_requested(&credential)),
        runtime_event(2, activated(&metadata)),
    ])
    .await;
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store.clone());

    registry
        .refresh_from_credential_stream_incremental(&stream, &event_store, 1)
        .await
        .unwrap();
    assert_eq!(
        resolver
            .resolve_plaintext(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_str(),
        "Bot token"
    );

    store.revoke(&credential).await.unwrap();
    event_store.push(credential.id().as_str(), 3, revoked(&credential));
    event_store.push(credential.id().as_str(), 4, destroy_requested(&credential));
    append_stream(
        &stream,
        StreamSubject::new("gateway.credentials.events.v1.raw").unwrap(),
        None,
        &[
            runtime_event(3, revoked(&credential)),
            runtime_event(4, destroy_requested(&credential)),
        ],
    )
    .await
    .unwrap();

    let report = registry
        .refresh_from_credential_stream_incremental(&stream, &event_store, 2)
        .await
        .unwrap();

    assert_eq!(report.scanned_events(), 3);
    assert_eq!(report.applied_credentials(), 1);
    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));

    registry
        .projections()
        .upsert(projection(RuntimeIntegrationStatus::Active, credential))
        .await;
    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::SecretStore(SecretStoreError::Unreadable { .. }))
    ));
}

#[tokio::test]
async fn incremental_refresh_invalidates_cache_and_removes_projection_when_batch_ends_in_cleanup_failed() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let event_store = ProjectionTestEventStore::default();
    event_store.push(credential.id().as_str(), 1, write_requested(&credential));
    event_store.push(credential.id().as_str(), 2, activated(&metadata));
    let stream = raw_stream([
        runtime_event(1, write_requested(&credential)),
        runtime_event(2, activated(&metadata)),
    ])
    .await;
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store.clone());

    registry
        .refresh_from_credential_stream_incremental(&stream, &event_store, 1)
        .await
        .unwrap();
    assert_eq!(
        resolver
            .resolve_plaintext(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_str(),
        "Bot token"
    );

    store.revoke(&credential).await.unwrap();
    event_store.push(credential.id().as_str(), 3, revoked(&credential));
    event_store.push(credential.id().as_str(), 4, destroy_requested(&credential));
    event_store.push(credential.id().as_str(), 5, destroy_failed(&credential));
    append_stream(
        &stream,
        StreamSubject::new("gateway.credentials.events.v1.raw").unwrap(),
        None,
        &[
            runtime_event(3, revoked(&credential)),
            runtime_event(4, destroy_requested(&credential)),
            runtime_event(5, destroy_failed(&credential)),
        ],
    )
    .await
    .unwrap();

    let report = registry
        .refresh_from_credential_stream_incremental(&stream, &event_store, 2)
        .await
        .unwrap();

    assert_eq!(report.scanned_events(), 4);
    assert_eq!(report.applied_credentials(), 1);
    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));

    registry
        .projections()
        .upsert(projection(RuntimeIntegrationStatus::Active, credential))
        .await;
    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::SecretStore(SecretStoreError::Unreadable { .. }))
    ));
}
