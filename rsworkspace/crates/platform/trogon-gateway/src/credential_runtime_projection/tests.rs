use std::sync::{Arc, Mutex};

use crate::credential::commands::domain::{
    AllowedHosts, AllowedRuntimeServices, CredentialDestroyReason, CredentialFailureReason, CredentialMetadata,
    CredentialScope, CredentialVersion, InjectionLocation, InjectionLocations, RuntimeServiceId,
};
use crate::credential::commands::proto::{
    activated_to_proto, decode_scope_key, destroy_failed_to_proto, destroy_requested_to_proto, destroyed_to_proto,
    revoked_to_proto, rotated_to_proto, rotation_failed_to_proto, rotation_requested_to_proto, write_failed_to_proto,
    write_requested_to_proto,
};
use crate::credential::stream::{CredentialKey, CredentialSubject};
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
use trogon_nats::jetstream::MockJetStreamPublishMessage;
use trogon_std::log_capture::{CapturedEvents, LevelFilter};
use uuid::Uuid;

use super::*;

#[derive(Clone, Default)]
struct ProjectionTestEventStore {
    events: Arc<Mutex<Vec<StreamEvent>>>,
    failing_reads: Arc<Mutex<usize>>,
}

#[derive(Debug, thiserror::Error)]
#[error("projection test event store failed")]
struct ProjectionTestEventStoreError;

impl ProjectionTestEventStore {
    fn fail_next_reads(&self, count: usize) {
        *self.failing_reads.lock().unwrap() = count;
    }

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
        {
            let mut failing_reads = self.failing_reads.lock().unwrap();
            if *failing_reads > 0 {
                *failing_reads -= 1;
                return Err(ProjectionTestEventStoreError);
            }
        }
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
async fn refresh_from_sequence_zero_reads_the_stream_from_its_first_event() {
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

    let report = registry.refresh_from_credential_stream(&stream, 0).await.unwrap();

    assert_eq!(report.scanned_events(), 2);
    assert_eq!(report.projected_integrations(), 1);
}

#[tokio::test]
async fn tracked_refresh_advances_cursor_and_resumes_after_it() {
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
    let resolver = registry.resolver(store);

    let first = registry
        .refresh_from_credential_stream_tracked(&stream, &event_store)
        .await
        .unwrap();
    let second = registry
        .refresh_from_credential_stream_tracked(&stream, &event_store)
        .await
        .unwrap();

    assert_eq!(first.scanned_events(), 2);
    assert_eq!(first.checkpoint_loaded_sequence(), 0);
    assert_eq!(first.checkpoint_advanced_to(), Some(2));
    assert_eq!(second.scanned_events(), 0);
    assert_eq!(second.checkpoint_loaded_sequence(), 2);
    assert_eq!(second.checkpoint_advanced_to(), None);
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
async fn fresh_registry_projects_credential_activated_before_another_registry_advanced_its_cursor() {
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
    let previous_process = RuntimeCredentialRegistry::default();
    previous_process
        .refresh_from_credential_stream_tracked(&stream, &event_store)
        .await
        .unwrap();

    let restarted = RuntimeCredentialRegistry::default();
    let resolver = restarted.resolver(store);
    let report = restarted
        .refresh_from_credential_stream_tracked(&stream, &event_store)
        .await
        .unwrap();

    assert_eq!(report.scanned_events(), 2);
    assert_eq!(report.checkpoint_loaded_sequence(), 0);
    assert_eq!(report.applied_credentials(), 1);
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
async fn independent_registries_each_observe_a_revocation_regardless_of_the_others_progress() {
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
    let replica_a = RuntimeCredentialRegistry::default();
    let replica_b = RuntimeCredentialRegistry::default();
    let resolver_a = replica_a.resolver(store.clone());
    let resolver_b = replica_b.resolver(store.clone());
    replica_a
        .refresh_from_credential_stream_tracked(&stream, &event_store)
        .await
        .unwrap();
    replica_b
        .refresh_from_credential_stream_tracked(&stream, &event_store)
        .await
        .unwrap();
    resolver_b.resolve(&key(), CredentialKind::BotToken).await.unwrap();

    store.revoke(&credential).await.unwrap();
    event_store.push(credential.id().as_str(), 3, revoked(&credential));
    append_stream(
        &stream,
        StreamSubject::new("gateway.credentials.events.v1.raw").unwrap(),
        None,
        &[runtime_event(3, revoked(&credential))],
    )
    .await
    .unwrap();

    let report_a = replica_a
        .refresh_from_credential_stream_tracked(&stream, &event_store)
        .await
        .unwrap();
    assert_eq!(report_a.checkpoint_advanced_to(), Some(3));
    assert!(matches!(
        resolver_a.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));

    let report_b = replica_b
        .refresh_from_credential_stream_tracked(&stream, &event_store)
        .await
        .unwrap();
    assert_eq!(report_b.scanned_events(), 1);
    assert_eq!(report_b.checkpoint_loaded_sequence(), 2);
    assert!(matches!(
        resolver_b.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));
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
async fn a_stale_active_state_does_not_republish_a_revoked_credential() {
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
    let revoked_state = evolve(active_state.clone(), &revoked(&credential)).unwrap();

    registry.apply_state(&revoked_state, position(3)).await.unwrap();
    registry.apply_state(&active_state, position(2)).await.unwrap();

    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));
}

#[tokio::test]
async fn a_stale_pending_write_state_does_not_evict_a_live_credential() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store.clone());
    let pending_state = build_state([write_requested(&credential)]);
    let active_state = evolve(
        pending_state.clone(),
        &v1::CredentialEvent {
            event: Some(activated_to_proto(&metadata).into()),
        },
    )
    .unwrap();

    registry.apply_state(&active_state, position(2)).await.unwrap();
    registry.apply_state(&pending_state, position(1)).await.unwrap();

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
async fn activating_one_credential_leaves_other_cached_secrets_in_place() {
    let store = MockOpenBaoSecretStore::default();
    let first = put_bot_token(&store, "Bot first").await;
    let second = store
        .put(
            CredentialScope::integration(
                owner_id(),
                SourceKind::Discord,
                SourceIntegrationId::new("secondary").unwrap(),
            ),
            CredentialKind::BotToken,
            SecretString::new("Bot second").unwrap(),
        )
        .await
        .unwrap();
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store.clone());
    let first_state = build_state([
        write_requested(&first),
        activated(&store.metadata(&first).await.unwrap()),
    ]);
    registry.apply_state(&first_state, position(2)).await.unwrap();
    resolver
        .resolve_plaintext(&key(), CredentialKind::BotToken)
        .await
        .unwrap();
    assert!(registry.cache().get(&first).await.is_some());

    let second_state = build_state([
        write_requested(&second),
        activated(&store.metadata(&second).await.unwrap()),
    ]);
    registry.apply_state(&second_state, position(2)).await.unwrap();

    assert!(registry.cache().get(&first).await.is_some());
}

#[tokio::test]
async fn applying_a_rotated_state_invalidates_the_replaced_credential() {
    let store = MockOpenBaoSecretStore::default();
    let previous = put_bot_token(&store, "Bot old-token").await;
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store.clone());
    let active_state = build_state([
        write_requested(&previous),
        activated(&store.metadata(&previous).await.unwrap()),
    ]);
    registry.apply_state(&active_state, position(2)).await.unwrap();
    resolver
        .resolve_plaintext(&key(), CredentialKind::BotToken)
        .await
        .unwrap();
    assert!(registry.cache().get(&previous).await.is_some());

    let rotated = store
        .rotate(&previous, SecretString::new("Bot new-token").unwrap())
        .await
        .unwrap();
    let pending_state = evolve(
        active_state,
        &v1::CredentialEvent {
            event: Some(rotation_requested_to_proto(&previous).into()),
        },
    )
    .unwrap();
    let rotated_state = evolve(
        pending_state,
        &v1::CredentialEvent {
            event: Some(rotated_to_proto(&previous, &store.metadata(&rotated).await.unwrap()).into()),
        },
    )
    .unwrap();
    registry.apply_state(&rotated_state, position(3)).await.unwrap();

    assert!(registry.cache().get(&previous).await.is_none());
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
async fn incremental_refresh_stops_serving_a_destroyed_credential_once_it_is_rewritten() {
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

    let destroyed = v1::CredentialEvent {
        event: Some(destroyed_to_proto(&credential).into()),
    };
    let rewrite = [
        revoked(&credential),
        destroy_requested(&credential),
        destroyed,
        write_requested(&credential),
    ];
    for (offset, event) in rewrite.iter().enumerate() {
        event_store.push(credential.id().as_str(), 3 + offset as u64, event.clone());
    }
    let runtime_events = rewrite
        .iter()
        .enumerate()
        .map(|(offset, event)| runtime_event(3 + offset as u64, event.clone()))
        .collect::<Vec<_>>();
    append_stream(
        &stream,
        StreamSubject::new("gateway.credentials.events.v1.raw").unwrap(),
        None,
        &runtime_events,
    )
    .await
    .unwrap();

    registry
        .refresh_from_credential_stream_incremental(&stream, &event_store, 2)
        .await
        .unwrap();

    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
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

fn subject_of(credential: &CredentialRef) -> String {
    CredentialSubject::event(&CredentialKey::derive(credential.id()))
        .unwrap()
        .as_str()
        .to_string()
}

async fn publish(stream: &MockJetStreamPublishMessage, subject: &str, events: &[Event]) {
    append_stream(stream, StreamSubject::new(subject).unwrap(), None, events)
        .await
        .unwrap();
}

fn undecodable_runtime_event(id: u64) -> Event {
    Event {
        content: vec![0xff, 0xff, 0xff, 0xff],
        ..runtime_event(id, write_requested(&cache_credential("poison")))
    }
}

#[tokio::test]
async fn incremental_refresh_skips_an_undecodable_event_and_applies_later_credentials() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let event_store = ProjectionTestEventStore::default();
    event_store.push(credential.id().as_str(), 1, write_requested(&credential));
    event_store.push(credential.id().as_str(), 2, activated(&metadata));
    let stream = MockJetStreamPublishMessage::new();
    publish(
        &stream,
        &subject_of(&cache_credential("poison")),
        &[undecodable_runtime_event(1)],
    )
    .await;
    publish(
        &stream,
        &subject_of(&credential),
        &[
            runtime_event(2, write_requested(&credential)),
            runtime_event(3, activated(&metadata)),
        ],
    )
    .await;
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store);

    let report = registry
        .refresh_from_credential_stream_incremental(&stream, &event_store, 1)
        .await
        .unwrap();

    assert_eq!(report.poisoned_events(), 1);
    assert_eq!(report.checkpoint_advanced_to(), Some(3));
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
async fn incremental_refresh_evicts_a_credential_whose_history_cannot_be_replayed() {
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
    resolver
        .resolve_plaintext(&key(), CredentialKind::BotToken)
        .await
        .unwrap();
    assert!(registry.cache().get(&credential).await.is_some());

    let rotated = v1::CredentialEvent {
        event: Some(rotated_to_proto(&credential, &metadata).into()),
    };
    event_store.push(credential.id().as_str(), 3, rotated.clone());
    append_stream(
        &stream,
        StreamSubject::new("gateway.credentials.events.v1.raw").unwrap(),
        None,
        &[runtime_event(3, rotated)],
    )
    .await
    .unwrap();

    let report = registry
        .refresh_from_credential_stream_incremental(&stream, &event_store, 3)
        .await
        .unwrap();

    assert_eq!(report.poisoned_credentials(), 1);
    assert_eq!(report.checkpoint_advanced_to(), Some(3));
    assert!(registry.cache().get(&credential).await.is_none());
    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));
}

#[tokio::test]
async fn incremental_refresh_evicts_a_served_credential_whose_new_event_cannot_be_decoded() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let event_store = ProjectionTestEventStore::default();
    event_store.push(credential.id().as_str(), 1, write_requested(&credential));
    event_store.push(credential.id().as_str(), 2, activated(&metadata));
    let stream = MockJetStreamPublishMessage::new();
    publish(
        &stream,
        &subject_of(&credential),
        &[
            runtime_event(1, write_requested(&credential)),
            runtime_event(2, activated(&metadata)),
        ],
    )
    .await;
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store);
    registry
        .refresh_from_credential_stream_incremental(&stream, &event_store, 1)
        .await
        .unwrap();
    resolver
        .resolve_plaintext(&key(), CredentialKind::BotToken)
        .await
        .unwrap();
    publish(&stream, &subject_of(&credential), &[undecodable_runtime_event(3)]).await;

    let report = registry
        .refresh_from_credential_stream_incremental(&stream, &event_store, 3)
        .await
        .unwrap();

    assert_eq!(report.poisoned_events(), 1);
    assert_eq!(report.poisoned_credentials(), 1);
    assert_eq!(report.checkpoint_advanced_to(), Some(3));
    assert!(matches!(
        resolver.resolve(&key(), CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));
}

#[tokio::test]
async fn incremental_refresh_stops_at_an_undecodable_event_that_belongs_to_no_credential_stream() {
    let event_store = ProjectionTestEventStore::default();
    let stream = raw_stream([undecodable_runtime_event(1)]).await;
    let registry = RuntimeCredentialRegistry::default();

    let error = registry
        .refresh_from_credential_stream_incremental(&stream, &event_store, 1)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        RuntimeProjectionStreamRefreshError::Refresh {
            source: RuntimeProjectionRefreshError::UnattributableEvent { stream_position: 1, .. }
        }
    ));
}

#[derive(Clone)]
struct VerifierOnlySecretStore;

impl SecretStoreGet for VerifierOnlySecretStore {
    type Error = SecretStoreError;

    async fn get(&self, _credential: &CredentialRef) -> Result<SecretMaterial, Self::Error> {
        Ok(SecretMaterial::Verifier(SecretVerifier::new("verifier").unwrap()))
    }
}

fn foreign_runtime_event(id: u64) -> Event {
    Event {
        r#type: "example.foreign.event".to_string(),
        ..runtime_event(id, write_requested(&cache_credential("foreign")))
    }
}

fn runtime_event_without_a_case(id: u64) -> Event {
    Event {
        content: Vec::new(),
        ..runtime_event(id, write_requested(&cache_credential("poison")))
    }
}

fn terminate_current_process() {
    let status = std::process::Command::new("kill")
        .args(["-TERM", &std::process::id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

fn messages(events: &CapturedEvents, message: &str) -> usize {
    events
        .events()
        .iter()
        .filter(|event| event.message() == Some(message))
        .count()
}

async fn stop_worker_once(until: impl Fn() -> bool) {
    while !until() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    loop {
        terminate_current_process();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn run_until_stopped(worker: impl std::future::Future<Output = ()>, stop: impl std::future::Future<Output = ()>) {
    tokio::time::timeout(Duration::from_secs(30), async {
        tokio::select! {
            () = worker => {}
            () = stop => {}
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn resolving_twice_serves_the_second_request_from_the_cache() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let registry = RuntimeCredentialRegistry::default();
    registry
        .apply_state(
            &build_state([
                write_requested(&credential),
                activated(&store.metadata(&credential).await.unwrap()),
            ]),
            position(2),
        )
        .await
        .unwrap();
    let resolver = registry.resolver(store);

    let first = resolver
        .resolve_plaintext(&key(), CredentialKind::BotToken)
        .await
        .unwrap();
    assert!(registry.cache().get(&credential).await.is_some());
    let second = resolver
        .resolve_plaintext(&key(), CredentialKind::BotToken)
        .await
        .unwrap();

    assert_eq!(first.as_str(), "Bot token");
    assert_eq!(second.as_str(), "Bot token");
}

#[tokio::test]
async fn resolve_plaintext_rejects_a_credential_that_only_has_a_verifier() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let projections = InMemoryRuntimeProjectionRepository::default();
    projections
        .upsert(projection(RuntimeIntegrationStatus::Active, credential))
        .await;
    let resolver = RuntimeCredentialResolver::new(projections, VerifierOnlySecretStore);

    let error = resolver
        .resolve_plaintext(&key(), CredentialKind::BotToken)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        RuntimeCredentialError::VerifierOnly {
            kind: CredentialKind::BotToken,
            ..
        }
    ));
}

#[test]
fn runtime_credential_errors_say_whether_they_came_from_the_secret_store() {
    let credential = cache_credential("credential-error");

    assert!(RuntimeCredentialError::SecretStore(SecretStoreError::Missing { credential }).is_secret_store_error());
    assert!(!RuntimeCredentialError::IntegrationNotFound { key: key() }.is_secret_store_error());
}

#[test]
fn integration_keys_display_their_source_and_integration() {
    assert_eq!(
        key().to_string(),
        format!("{}/{}", SourceKind::Discord, integration_id().as_str())
    );
    assert_eq!(source_key().to_string(), SourceKind::Discord.to_string());
    assert_eq!(key().integration_id(), Some(integration_id().as_str()));
}

#[test]
fn cache_policy_exposes_its_ttl_and_jitter() {
    let policy = RuntimeCredentialCachePolicy::new(Duration::from_secs(60), Duration::from_secs(5)).unwrap();

    assert_eq!(policy.ttl(), Duration::from_secs(60));
    assert_eq!(policy.jitter(), Duration::from_secs(5));
}

#[tokio::test]
async fn cache_without_jitter_expires_exactly_at_the_ttl() {
    let policy = RuntimeCredentialCachePolicy::new(Duration::from_secs(60), Duration::ZERO).unwrap();
    let cache = RuntimeCredentialCache::with_policy(policy);
    let credential = cache_credential("credential-no-jitter");
    let now = Instant::now();

    cache.put_at(credential.clone(), cache_secret("token"), now).await;

    assert!(cache.get_at(&credential, now + Duration::from_secs(59)).await.is_some());
    assert!(cache.get_at(&credential, now + Duration::from_secs(60)).await.is_none());
}

#[tokio::test]
async fn an_active_credential_with_a_pending_rotation_keeps_serving_its_active_version() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let state = build_state([
        write_requested(&credential),
        activated(&metadata),
        v1::CredentialEvent {
            event: Some(rotation_requested_to_proto(&credential).into()),
        },
    ]);

    let projection = RuntimeIntegrationProjection::from_credential_state(&state, 3)
        .unwrap()
        .unwrap();

    assert_eq!(projection.key(), &key());
    assert_eq!(projection.version(), 3);
    assert_eq!(projection.credential(CredentialKind::BotToken), Some(&credential));
}

#[test]
fn the_refresh_report_counts_any_observed_work_as_activity() {
    assert!(!RuntimeProjectionRefreshReport::default().has_projection_activity());
    for report in [
        RuntimeProjectionRefreshReport {
            scanned_events: 1,
            ..Default::default()
        },
        RuntimeProjectionRefreshReport {
            decoded_events: 1,
            ..Default::default()
        },
        RuntimeProjectionRefreshReport {
            skipped_events: 1,
            ..Default::default()
        },
        RuntimeProjectionRefreshReport {
            poisoned_events: 1,
            ..Default::default()
        },
        RuntimeProjectionRefreshReport {
            poisoned_credentials: 1,
            ..Default::default()
        },
        RuntimeProjectionRefreshReport {
            changed_credentials: 1,
            ..Default::default()
        },
        RuntimeProjectionRefreshReport {
            applied_credentials: 1,
            ..Default::default()
        },
        RuntimeProjectionRefreshReport {
            projected_integrations: 1,
            ..Default::default()
        },
        RuntimeProjectionRefreshReport {
            checkpoint_advanced_to: Some(1),
            ..Default::default()
        },
    ] {
        assert!(report.has_projection_activity());
    }
}

#[tokio::test]
async fn the_repository_can_remove_merge_and_count_projections() {
    let store = MockOpenBaoSecretStore::default();
    let first = put_bot_token(&store, "first").await;
    let rotated = store
        .rotate(&first, SecretString::new("second").unwrap())
        .await
        .unwrap();
    let projections = InMemoryRuntimeProjectionRepository::default();

    assert!(
        projections
            .merge(projection(RuntimeIntegrationStatus::Active, first.clone()))
            .await
            .unwrap()
            .is_empty()
    );
    let replaced = projections
        .merge(projection(RuntimeIntegrationStatus::Active, rotated.clone()))
        .await
        .unwrap();
    assert_eq!(replaced, vec![first]);
    assert_eq!(projections.len().await, 1);

    assert_eq!(
        projections.remove_credential(&key(), CredentialKind::BotToken).await,
        Some(rotated)
    );
    projections
        .upsert(projection(
            RuntimeIntegrationStatus::Active,
            put_bot_token(&store, "third").await,
        ))
        .await;
    projections.remove(&key()).await;
    assert_eq!(projections.len().await, 0);
}

#[tokio::test]
async fn merging_a_projection_for_another_owner_is_refused() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "token").await;
    let projections = InMemoryRuntimeProjectionRepository::default();
    projections
        .merge(projection(RuntimeIntegrationStatus::Active, credential.clone()))
        .await
        .unwrap();
    let other_owner = RuntimeIntegrationProjection::new(
        CredentialOwnerId::new("tenant-2").unwrap(),
        SourceKind::Discord,
        integration_id(),
        RuntimeIntegrationStatus::Active,
        1,
    )
    .with_credential(CredentialKind::BotToken, credential);

    let error = projections.merge(other_owner).await.unwrap_err();

    assert!(matches!(error, RuntimeProjectionRefreshError::OwnerMismatch { .. }));
}

#[tokio::test]
async fn replacing_all_projections_tolerates_a_projection_with_no_stream_position() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "token").await;
    let unversioned = RuntimeIntegrationProjection::new(
        owner_id(),
        SourceKind::Discord,
        integration_id(),
        RuntimeIntegrationStatus::Active,
        0,
    )
    .with_credential(CredentialKind::BotToken, credential);
    let projections = InMemoryRuntimeProjectionRepository::default();

    projections
        .replace_all(BTreeMap::from([(unversioned.key().clone(), unversioned)]))
        .await;

    assert_eq!(projections.len().await, 1);
}

#[tokio::test]
async fn full_refresh_counts_events_of_other_types_as_skipped() {
    let registry = RuntimeCredentialRegistry::default();
    let mut foreign = stream_event("ignored", 1, write_requested(&cache_credential("foreign")));
    foreign.event = foreign_runtime_event(1);

    let report = registry.refresh_from_credential_events([foreign]).await.unwrap();

    assert_eq!(report.scanned_events(), 1);
    assert_eq!(report.skipped_events(), 1);
    assert_eq!(report.decoded_events(), 0);
}

#[tokio::test]
async fn incremental_refresh_counts_events_of_other_types_as_skipped() {
    let event_store = ProjectionTestEventStore::default();
    let stream = raw_stream([foreign_runtime_event(1)]).await;
    let registry = RuntimeCredentialRegistry::default();

    let report = registry
        .refresh_from_credential_stream_incremental(&stream, &event_store, 1)
        .await
        .unwrap();

    assert_eq!(report.skipped_events(), 1);
    assert_eq!(report.poisoned_events(), 0);
    assert_eq!(report.checkpoint_advanced_to(), Some(1));
}

#[tokio::test]
async fn incremental_refresh_poisons_an_event_that_carries_no_event_case() {
    let event_store = ProjectionTestEventStore::default();
    let stream = MockJetStreamPublishMessage::new();
    publish(
        &stream,
        &subject_of(&cache_credential("poison")),
        &[runtime_event_without_a_case(1)],
    )
    .await;
    let registry = RuntimeCredentialRegistry::default();

    let report = registry
        .refresh_from_credential_stream_incremental(&stream, &event_store, 1)
        .await
        .unwrap();

    assert_eq!(report.decoded_events(), 1);
    assert_eq!(report.poisoned_events(), 1);
    assert_eq!(report.changed_credentials(), 0);
}

#[tokio::test]
async fn incremental_refresh_ignores_other_event_types_in_a_credential_history() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let event_store = ProjectionTestEventStore::default();
    event_store.push(credential.id().as_str(), 1, write_requested(&credential));
    let mut foreign = stream_event(credential.id().as_str(), 2, write_requested(&credential));
    foreign.event = foreign_runtime_event(2);
    event_store.events.lock().unwrap().push(foreign);
    event_store.push(credential.id().as_str(), 3, activated(&metadata));
    let stream = raw_stream([
        runtime_event(1, write_requested(&credential)),
        runtime_event(3, activated(&metadata)),
    ])
    .await;
    let registry = RuntimeCredentialRegistry::default();

    registry
        .refresh_from_credential_stream_incremental(&stream, &event_store, 1)
        .await
        .unwrap();

    assert_eq!(
        registry
            .resolver(store)
            .resolve_plaintext(&key(), CredentialKind::BotToken)
            .await
            .unwrap()
            .as_str(),
        "Bot token"
    );
}

#[tokio::test]
async fn incremental_refresh_reports_a_credential_history_that_cannot_be_read() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let event_store = ProjectionTestEventStore::default();
    event_store.fail_next_reads(1);
    let stream = raw_stream([runtime_event(1, write_requested(&credential))]).await;
    let registry = RuntimeCredentialRegistry::default();

    let error = registry
        .refresh_from_credential_stream_incremental(&stream, &event_store, 1)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        RuntimeProjectionStreamRefreshError::Refresh {
            source: RuntimeProjectionRefreshError::ReadCredential { .. }
        }
    ));
}

#[tokio::test]
async fn applying_a_state_with_nothing_to_publish_changes_nothing() {
    let registry = RuntimeCredentialRegistry::default();

    registry.apply_state(&initial_state(), position(1)).await.unwrap();

    assert!(registry.projections.get(&key()).await.is_none());
}

#[tokio::test]
async fn applying_a_pending_write_or_failed_write_for_an_unprojected_credential_is_harmless() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let reason = CredentialFailureReason::new("backend refused").unwrap();
    let registry = RuntimeCredentialRegistry::default();
    let pending = build_state([write_requested(&credential)]);
    let failed = build_state([
        write_requested(&credential),
        v1::CredentialEvent {
            event: Some(write_failed_to_proto(credential.id(), &reason).into()),
        },
    ]);

    registry.apply_state(&pending, position(1)).await.unwrap();
    registry.apply_state(&failed, position(2)).await.unwrap();

    assert!(registry.projections.get(&key()).await.is_none());
}

#[tokio::test]
async fn applying_a_state_whose_credential_id_is_invalid_is_refused() {
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let reason = CredentialFailureReason::new("backend refused").unwrap();
    let registry = RuntimeCredentialRegistry::default();
    let mut pending = build_state([write_requested(&credential)]);
    let mut failed = build_state([
        write_requested(&credential),
        v1::CredentialEvent {
            event: Some(write_failed_to_proto(credential.id(), &reason).into()),
        },
    ]);
    let Some(CredentialStateSnapshotCase::PendingWrite(inner)) = pending.state.as_mut() else {
        panic!("expected a pending write");
    };
    inner.credential_id.clear();
    let Some(CredentialStateSnapshotCase::WriteFailed(inner)) = failed.state.as_mut() else {
        panic!("expected a failed write");
    };
    inner.credential_id.clear();

    for state in [pending, failed] {
        let error = registry.apply_state(&state, position(1)).await.unwrap_err();
        assert!(matches!(error, RuntimeProjectionRefreshError::InvalidEvent { .. }));
    }
}

#[test]
fn every_credential_event_kind_names_the_credential_it_belongs_to() {
    let store_credential = CredentialRef::new(
        CredentialId::new("openbao:tenant-1:discord:bot_token").unwrap(),
        CredentialVersion::initial(),
        &CredentialScope::source(owner_id(), SourceKind::Discord),
        CredentialKind::BotToken,
    );
    let metadata = CredentialMetadata::new(
        store_credential.clone(),
        crate::credential::commands::domain::CredentialStatus::Active,
        crate::credential::commands::domain::StorageBackend::OpenBao,
        crate::credential::commands::domain::CredentialFingerprint::new("fingerprint").unwrap(),
    );
    let reason = CredentialFailureReason::new("backend refused").unwrap();
    let events = [
        write_requested(&store_credential),
        v1::CredentialEvent {
            event: Some(write_failed_to_proto(store_credential.id(), &reason).into()),
        },
        activated(&metadata),
        v1::CredentialEvent {
            event: Some(rotation_requested_to_proto(&store_credential).into()),
        },
        v1::CredentialEvent {
            event: Some(rotation_failed_to_proto(&store_credential, &reason).into()),
        },
        revoked(&store_credential),
        destroy_requested(&store_credential),
        destroy_failed(&store_credential),
        v1::CredentialEvent {
            event: Some(destroyed_to_proto(&store_credential).into()),
        },
        v1::CredentialEvent {
            event: Some(rotated_to_proto(&store_credential, &metadata).into()),
        },
    ];

    for event in &events {
        assert_eq!(
            event_credential_id(event.event.as_ref().unwrap()).unwrap(),
            *store_credential.id()
        );
    }
}

#[tokio::test]
async fn the_refresh_worker_stays_quiet_while_there_is_nothing_to_project() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let registry = RuntimeCredentialRegistry::default();
    let event_store = ProjectionTestEventStore::default();
    let stream = raw_stream([]).await;
    let worker = run_tracked_refresh_worker(registry, stream, event_store, Duration::from_millis(5));
    let started = Instant::now();

    run_until_stopped(
        worker,
        stop_worker_once(|| started.elapsed() > Duration::from_millis(100)),
    )
    .await;

    assert_eq!(
        messages(&captured, "credential runtime projection refresh worker stopped"),
        1
    );
    assert_eq!(
        messages(&captured, "credential runtime projection refresh pass completed"),
        0
    );
    assert_eq!(
        messages(&captured, "credential runtime projection refresh pass failed"),
        0
    );
}

#[tokio::test]
async fn the_refresh_worker_retries_after_a_failed_pass_and_logs_the_projection() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let store = MockOpenBaoSecretStore::default();
    let credential = put_bot_token(&store, "Bot token").await;
    let metadata = store.metadata(&credential).await.unwrap();
    let event_store = ProjectionTestEventStore::default();
    event_store.push(credential.id().as_str(), 1, write_requested(&credential));
    event_store.push(credential.id().as_str(), 2, activated(&metadata));
    event_store.fail_next_reads(1);
    let stream = raw_stream([
        runtime_event(1, write_requested(&credential)),
        runtime_event(2, activated(&metadata)),
    ])
    .await;
    let registry = RuntimeCredentialRegistry::default();
    let resolver = registry.resolver(store);
    let worker = run_tracked_refresh_worker(registry, stream, event_store, Duration::from_millis(5));

    run_until_stopped(
        worker,
        stop_worker_once(|| messages(&captured, "credential runtime projection refresh pass completed") > 0),
    )
    .await;

    assert_eq!(
        messages(&captured, "credential runtime projection refresh pass failed"),
        1
    );
    assert_eq!(
        messages(&captured, "credential runtime projection refresh worker stopped"),
        1
    );
    let completed = captured
        .events()
        .into_iter()
        .find(|event| event.message() == Some("credential runtime projection refresh pass completed"))
        .unwrap();
    assert_eq!(completed.field("projected_integrations"), Some("1"));
    assert_eq!(completed.field("checkpoint_advanced_to"), Some("Some(2)"));
    assert!(resolver.resolve(&key(), CredentialKind::BotToken).await.is_ok());
}
