use buffa::MessageField;
use trogon_decider_runtime::{
    AppendFailure, AppendStreamRequest, Event, EventEncode, EventId, EventType, Headers, ReadFrom, ReadStreamRequest,
    StreamAppend, StreamRead, StreamWritePrecondition,
};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_peer::{CallerId, EnrollmentIdentity, InstallationId, NativeSessionId, TenantId, TrustedPeerOwner};
use trogon_peer_nats::{
    PeerTenant,
    peer::{PeerJetStreamStore, PeerStorageBudget, PeerStoreConfig},
};
use trogon_peer_service::ScopedPeerStore;
use trogonai_proto::peers::v1alpha1 as proto;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
fn event(index: u128) -> Result<Event, Box<dyn std::error::Error + Send + Sync>> {
    let event = proto::PeerEvent {
        event: Some(
            proto::PeerBatchAcknowledged {
                batch_id: Some(format!("batch-{index}")),
                acknowledged_at: MessageField::some(buffa_types::google::protobuf::Timestamp::from_unix(42, 0)),
            }
            .into(),
        ),
    };
    Ok(Event {
        id: EventId::from(Uuid::from_u128(index)),
        r#type: event.event_type()?.to_owned(),
        content: EventEncode::encode(&event)?,
        headers: Headers::empty(),
    })
}
#[tokio::test]
async fn scoped_store_bounded_replay_and_conflict_classification_preserve_committed_events() -> TestResult {
    let broker = JetStreamTestServer::start_with_atomic_batches().await;
    let config = PeerStoreConfig::new(PeerTenant::new("alpha")?, PeerStorageBudget::new(65_536)?);
    let store = PeerJetStreamStore::open(broker.jetstream().await, &config).await?;
    assert_eq!(store.tenant().as_str(), "alpha");
    let identity = EnrollmentIdentity::new(
        TrustedPeerOwner::new(TenantId::new("alpha")?, CallerId::new("alice")?),
        InstallationId::new("install")?,
        NativeSessionId::new("session")?,
    );
    let peer = identity.peer_id();
    let events = vec![event(1)?, event(2)?];
    let committed = store
        .append_stream(AppendStreamRequest {
            stream_id: peer,
            stream_write_precondition: StreamWritePrecondition::NoStream,
            events: events.clone(),
        })
        .await?;
    let bounded = store
        .read_stream_bounded(
            ReadStreamRequest {
                stream_id: peer,
                from: ReadFrom::Beginning,
            },
            1,
        )
        .await?;
    assert_eq!(bounded.events.len(), 1);
    assert_eq!(bounded.events[0].event, events[0]);
    assert_eq!(bounded.current_position, Some(committed.stream_position));
    let rejected = store
        .append_stream(AppendStreamRequest {
            stream_id: peer,
            stream_write_precondition: StreamWritePrecondition::NoStream,
            events: vec![event(3)?],
        })
        .await;
    let error = match rejected {
        Err(error) => error,
        Ok(_) => return Err("stale creator overwrote committed history".into()),
    };
    assert_eq!(store.classify_append_failure(&error), AppendFailure::WriteConflict);
    let retained = store
        .read_stream(ReadStreamRequest {
            stream_id: peer,
            from: ReadFrom::Beginning,
        })
        .await?;
    assert_eq!(retained.current_position, Some(committed.stream_position));
    assert_eq!(retained.events.into_iter().map(|e| e.event).collect::<Vec<_>>(), events);
    Ok(())
}
