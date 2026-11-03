#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_nats::jetstream::stream::{self, DiscardPolicy, RetentionPolicy, StorageType};
use buffa::MessageField;
use tokio::sync::Barrier;
use trogon_decider_nats::{JetStreamStore, JetStreamStoreError, StreamSubjectResolver, SubjectScope, SubjectState};
use trogon_decider_runtime::{
    AppendFailure, AppendStreamRequest, Event, EventEncode, EventId, EventType, Headers, ReadFrom, ReadStreamRequest,
    StreamAppend, StreamPosition, StreamRead, StreamWritePrecondition,
};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_peer::{CallerId, EnrollmentIdentity, InstallationId, NativeSessionId, PeerId, TenantId, TrustedPeerOwner};
use trogonai_proto::peers::v1alpha1 as proto;
use uuid::Uuid;

use super::{
    PeerStorageBudget, PeerStorageBudgetError, PeerStoreConfig, PeerStoreError, PeerStreamConfigMismatch,
    PeerSubjectError, PeerSubjectResolver, open_store,
};
use crate::PeerTenant;

fn peer_id() -> PeerId {
    EnrollmentIdentity::new(
        TrustedPeerOwner::new(
            TenantId::new("tenant_alpha").unwrap(),
            CallerId::new("caller_alpha").unwrap(),
        ),
        InstallationId::new("installation_alpha").unwrap(),
        NativeSessionId::new("session_alpha").unwrap(),
    )
    .peer_id()
    .clone()
}

fn config(tenant: &str) -> PeerStoreConfig {
    PeerStoreConfig::new(
        PeerTenant::new(tenant).unwrap(),
        PeerStorageBudget::new(65_536).unwrap(),
    )
}

fn event(index: u128) -> Event {
    let event = proto::PeerEvent {
        event: Some(
            proto::PeerBatchAcknowledged {
                batch_id: Some(format!("batch_{index}")),
                acknowledged_at: MessageField::some(buffa_types::google::protobuf::Timestamp::from_unix(42, 0)),
            }
            .into(),
        ),
    };
    Event {
        id: EventId::from(Uuid::from_u128(index)),
        r#type: event.event_type().unwrap().to_owned(),
        content: EventEncode::encode(&event).unwrap(),
        headers: Headers::empty(),
    }
}

#[derive(Clone)]
struct CoordinatedResolver {
    inner: PeerSubjectResolver,
    ready: Arc<Barrier>,
    observed_positions: Arc<Mutex<Vec<Option<StreamPosition>>>>,
}

impl StreamSubjectResolver<PeerId> for CoordinatedResolver {
    type Error = PeerSubjectError;

    fn subject_scope(&self) -> Option<&SubjectScope> {
        self.inner.subject_scope()
    }

    async fn resolve_subject_state(
        &self,
        events_stream: &async_nats::jetstream::stream::Stream,
        stream_id: &PeerId,
    ) -> Result<SubjectState, Self::Error> {
        let state = self.inner.resolve_subject_state(events_stream, stream_id).await?;
        self.observed_positions.lock().unwrap().push(state.current_position);
        self.ready.wait().await;
        Ok(state)
    }
}

#[test]
fn storage_budget_rejects_unbounded_or_unrepresentable_values() {
    assert!(matches!(PeerStorageBudget::new(0), Err(PeerStorageBudgetError::Zero)));
    assert!(matches!(
        PeerStorageBudget::new((i64::MAX as u64) + 1),
        Err(PeerStorageBudgetError::OutOfRange)
    ));
    assert_eq!(PeerStorageBudget::new(65_536).unwrap().as_bytes(), 65_536);
}

#[test]
fn peer_stream_configuration_rejects_writes_instead_of_evicting_history() {
    let config = config("tenant_alpha");
    let stream = config.stream_config();

    assert_eq!(stream.name, "PEER_tenant_alpha");
    assert_eq!(stream.subjects, ["tenant_alpha.peer.v1.peer.*.events"]);
    assert_eq!(stream.retention, RetentionPolicy::Limits);
    assert_eq!(stream.storage, StorageType::File);
    assert_eq!(stream.discard, DiscardPolicy::New);
    assert_eq!(stream.max_bytes, 65_536);
    assert_eq!(stream.max_age, Duration::ZERO);
    assert_eq!(stream.max_messages, -1);
    assert_eq!(stream.max_messages_per_subject, -1);
    assert!(stream.allow_atomic_publish);
    assert_eq!(config.snapshot_config().bucket, "PEER_SNAPSHOTS_tenant_alpha");
}

#[test]
fn peer_subjects_are_derived_from_validated_tenant_and_peer_identity() {
    let id = peer_id();
    let alpha = PeerSubjectResolver::new(PeerTenant::new("tenant_alpha").unwrap()).unwrap();
    let beta = PeerSubjectResolver::new(PeerTenant::new("tenant_beta").unwrap()).unwrap();
    let subject = alpha.subject(&id).unwrap();

    assert_eq!(subject.as_str(), format!("tenant_alpha.peer.v1.peer.{id}.events"));
    assert!(alpha.subject_scope().unwrap().contains(&subject));
    assert!(!beta.subject_scope().unwrap().contains(&subject));
    assert_eq!(alpha.tenant().as_str(), "tenant_alpha");
}

#[tokio::test]
async fn committed_atomic_batch_survives_a_new_store_handle() {
    let broker = JetStreamTestServer::start_with_atomic_batches().await;
    let js = broker.jetstream().await;
    let config = config("tenant_alpha");
    let id = peer_id();
    let store = open_store(js.clone(), &config).await.expect("open peer store");
    let events = vec![event(1), event(2)];
    let appended = store
        .append_stream(AppendStreamRequest {
            stream_id: &id,
            stream_write_precondition: StreamWritePrecondition::NoStream,
            events: events.clone(),
        })
        .await
        .expect("commit atomic peer batch");
    drop(store);

    let reopened = open_store(js, &config).await.expect("reopen peer store");
    let read = reopened
        .read_stream(ReadStreamRequest {
            stream_id: &id,
            from: ReadFrom::Beginning,
        })
        .await
        .expect("read committed peer events");

    assert_eq!(read.current_position, Some(appended.stream_position));
    assert_eq!(read.events.len(), 2);
    assert_eq!(
        read.events.iter().map(|stored| &stored.event).collect::<Vec<_>>(),
        events.iter().collect::<Vec<_>>()
    );
    for stored in read.events {
        assert!(stored.decode::<proto::PeerEvent>().unwrap().into_decoded().is_some());
    }
}

#[tokio::test]
async fn stale_expected_position_cannot_overwrite_a_committed_event() {
    let broker = JetStreamTestServer::start_with_atomic_batches().await;
    let config = config("tenant_alpha");
    let store = open_store(broker.jetstream().await, &config)
        .await
        .expect("open peer store");
    let id = peer_id();
    let first = store
        .append_stream(AppendStreamRequest {
            stream_id: &id,
            stream_write_precondition: StreamWritePrecondition::NoStream,
            events: vec![event(1)],
        })
        .await
        .expect("append initial event");
    let second = store
        .append_stream(AppendStreamRequest {
            stream_id: &id,
            stream_write_precondition: StreamWritePrecondition::At(first.stream_position),
            events: vec![event(2)],
        })
        .await
        .expect("advance peer stream");
    let error = store
        .append_stream(AppendStreamRequest {
            stream_id: &id,
            stream_write_precondition: StreamWritePrecondition::At(first.stream_position),
            events: vec![event(3)],
        })
        .await
        .expect_err("stale writer must be rejected");

    assert!(matches!(error, JetStreamStoreError::OptimisticConcurrencyConflict(_)));
    assert_eq!(store.classify_append_failure(&error), AppendFailure::WriteConflict);
    let read = store
        .read_stream(ReadStreamRequest {
            stream_id: &id,
            from: ReadFrom::Beginning,
        })
        .await
        .expect("read after rejected append");
    assert_eq!(read.current_position, Some(second.stream_position));
    assert_eq!(read.events.len(), 2);
}

#[tokio::test]
async fn concurrent_writers_with_the_same_observed_position_contend_at_the_broker() {
    let broker = JetStreamTestServer::start_with_atomic_batches().await;
    let config = config("tenant_alpha");
    let first_store = open_store(broker.jetstream().await, &config)
        .await
        .expect("open first peer store");
    let second_store = open_store(broker.jetstream().await, &config)
        .await
        .expect("open independent peer store");
    let id = peer_id();
    let initial = first_store
        .append_stream(AppendStreamRequest {
            stream_id: &id,
            stream_write_precondition: StreamWritePrecondition::NoStream,
            events: vec![event(1)],
        })
        .await
        .expect("seed peer stream");
    let ready = Arc::new(Barrier::new(2));
    let observed_positions = Arc::new(Mutex::new(Vec::new()));
    let left = JetStreamStore::builder(
        first_store.as_jetstream().clone(),
        first_store.events_stream().clone(),
        first_store.snapshot_bucket().clone(),
    )
    .with_subject_resolver(CoordinatedResolver {
        inner: first_store.subject_resolver().clone(),
        ready: ready.clone(),
        observed_positions: observed_positions.clone(),
    });
    let right = JetStreamStore::builder(
        second_store.as_jetstream().clone(),
        second_store.events_stream().clone(),
        second_store.snapshot_bucket().clone(),
    )
    .with_subject_resolver(CoordinatedResolver {
        inner: second_store.subject_resolver().clone(),
        ready,
        observed_positions: observed_positions.clone(),
    });

    let (left_result, right_result) = tokio::join!(
        left.append_stream(AppendStreamRequest {
            stream_id: &id,
            stream_write_precondition: StreamWritePrecondition::At(initial.stream_position),
            events: vec![event(2)],
        }),
        right.append_stream(AppendStreamRequest {
            stream_id: &id,
            stream_write_precondition: StreamWritePrecondition::At(initial.stream_position),
            events: vec![event(3)],
        }),
    );

    assert_eq!(
        *observed_positions.lock().unwrap(),
        vec![Some(initial.stream_position), Some(initial.stream_position)]
    );
    assert_ne!(left_result.is_ok(), right_result.is_ok());
    let (winner, conflict, winner_id, loser_id) = match left_result {
        Ok(winner) => (winner, right_result.unwrap_err(), event(2).id, event(3).id),
        Err(conflict) => (right_result.unwrap(), conflict, event(3).id, event(2).id),
    };
    assert!(matches!(
        conflict,
        JetStreamStoreError::OptimisticConcurrencyConflict(_)
    ));
    assert_eq!(left.classify_append_failure(&conflict), AppendFailure::WriteConflict);
    let read = first_store
        .read_stream(ReadStreamRequest {
            stream_id: &id,
            from: ReadFrom::Beginning,
        })
        .await
        .expect("read after broker contention");
    assert_eq!(read.current_position, Some(winner.stream_position));
    assert_eq!(read.events.len(), 2);
    assert!(read.events.iter().any(|stored| stored.event.id == winner_id));
    assert!(read.events.iter().all(|stored| stored.event.id != loser_id));
}

#[tokio::test]
async fn exhausted_byte_budget_rejects_new_writes_without_evicting_committed_events() {
    let broker = JetStreamTestServer::start_with_atomic_batches().await;
    let config = PeerStoreConfig::new(
        PeerTenant::new("tenant_alpha").unwrap(),
        PeerStorageBudget::new(2_048).unwrap(),
    );
    let store = open_store(broker.jetstream().await, &config)
        .await
        .expect("open bounded peer store");
    let id = peer_id();
    let mut committed = Vec::new();
    let mut position = None;
    let mut rejected = false;
    for index in 1..=64 {
        let next = event(index);
        match store
            .append_stream(AppendStreamRequest {
                stream_id: &id,
                stream_write_precondition: position.into(),
                events: vec![next.clone()],
            })
            .await
        {
            Ok(appended) => {
                committed.push(next);
                position = Some(appended.stream_position);
            }
            Err(error) => {
                assert_eq!(store.classify_append_failure(&error), AppendFailure::Fatal);
                rejected = true;
                break;
            }
        }
    }
    assert!(!committed.is_empty());
    assert!(rejected, "the finite byte budget must reject an append");
    let read = store
        .read_stream(ReadStreamRequest {
            stream_id: &id,
            from: ReadFrom::Beginning,
        })
        .await
        .expect("read retained events after capacity rejection");
    assert_eq!(read.current_position, position);
    assert_eq!(
        read.events.into_iter().map(|stored| stored.event).collect::<Vec<_>>(),
        committed
    );
}

#[tokio::test]
async fn distinct_tenant_stores_do_not_read_or_write_each_others_subjects() {
    let broker = JetStreamTestServer::start_with_atomic_batches().await;
    let js = broker.jetstream().await;
    let alpha = open_store(js.clone(), &config("tenant_alpha"))
        .await
        .expect("open alpha store");
    let beta = open_store(js, &config("tenant_beta")).await.expect("open beta store");
    let id = peer_id();
    alpha
        .append_stream(AppendStreamRequest {
            stream_id: &id,
            stream_write_precondition: StreamWritePrecondition::NoStream,
            events: vec![event(1)],
        })
        .await
        .expect("append alpha event");
    let empty = beta
        .read_stream(ReadStreamRequest {
            stream_id: &id,
            from: ReadFrom::Beginning,
        })
        .await
        .expect("read beta before its append");
    assert!(empty.events.is_empty());
    assert_eq!(empty.current_position, None);

    beta.append_stream(AppendStreamRequest {
        stream_id: &id,
        stream_write_precondition: StreamWritePrecondition::NoStream,
        events: vec![event(2)],
    })
    .await
    .expect("append beta event");
    let read = alpha
        .read_stream(ReadStreamRequest {
            stream_id: &id,
            from: ReadFrom::Beginning,
        })
        .await
        .expect("read alpha after beta append");
    assert_eq!(read.events.len(), 1);
    assert_eq!(read.events[0].event, event(1));
}

#[tokio::test]
async fn opening_an_evicting_stream_fails_without_reconfiguring_it() {
    let broker = JetStreamTestServer::start_with_atomic_batches().await;
    let js = broker.jetstream().await;
    let config = config("tenant_alpha");
    let unsafe_config = stream::Config {
        name: config.stream_name(),
        subjects: vec!["tenant_alpha.peer.v1.peer.*.events".into()],
        retention: RetentionPolicy::Limits,
        storage: StorageType::File,
        discard: DiscardPolicy::Old,
        max_bytes: 65_536,
        max_messages: -1,
        max_messages_per_subject: -1,
        allow_atomic_publish: true,
        ..Default::default()
    };
    js.create_stream(unsafe_config)
        .await
        .expect("create existing evicting stream");

    let result = open_store(js.clone(), &config).await;
    assert!(matches!(
        result,
        Err(PeerStoreError::UnsafeConfig(PeerStreamConfigMismatch::Discard))
    ));
    let existing = js.get_stream(config.stream_name()).await.expect("read existing stream");
    assert_eq!(existing.cached_info().config.discard, DiscardPolicy::Old);
}
