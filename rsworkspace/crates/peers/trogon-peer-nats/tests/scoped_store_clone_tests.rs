use trogon_decider_runtime::{ReadFrom, ReadStreamRequest, StreamRead};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_peer::PeerId;
use trogon_peer_nats::{
    PeerTenant,
    peer::{PeerJetStreamStore, PeerStorageBudget, PeerStoreConfig, open_store},
};
use trogon_peer_service::{PeerService, ScopedPeerStore, test_support::Fixture};

#[tokio::test]
async fn cloned_store_composes_a_service_and_shares_durable_history()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let broker = JetStreamTestServer::start_with_atomic_batches().await;
    let fixture = Fixture::new().await?;
    let config = PeerStoreConfig::new(PeerTenant::new("tenant")?, PeerStorageBudget::new(1_048_576)?);
    let store = PeerJetStreamStore::new(open_store(broker.jetstream().await, &config).await?)?;
    let service_store = store.clone();
    assert_eq!(service_store.tenant(), store.tenant());
    let service = PeerService::new(service_store, fixture.policy.clone(), fixture.clock.clone());
    let enrolled = service.enroll(&fixture.alice, Fixture::enrollment("alice")).await?;
    let peer = PeerId::parse(enrolled.peer_id.ok_or("missing enrolled peer")?)?;
    let original = store
        .read_stream(ReadStreamRequest {
            stream_id: &peer,
            from: ReadFrom::Beginning,
        })
        .await?;
    assert!(!original.events.is_empty());
    let cloned = store
        .clone()
        .read_stream(ReadStreamRequest {
            stream_id: &peer,
            from: ReadFrom::Beginning,
        })
        .await?;
    assert_eq!(original.current_position, cloned.current_position);
    assert_eq!(
        original.events.into_iter().map(|event| event.event).collect::<Vec<_>>(),
        cloned.events.into_iter().map(|event| event.event).collect::<Vec<_>>()
    );
    Ok(())
}
