use trogon_nats::test_support::JetStreamTestServer;
use trogon_peer_nats::{
    PeerTenant,
    peer::{PeerJetStreamStore, PeerStorageBudget, PeerStoreConfig, open_store},
};
use trogon_peer_service::ScopedPeerStore;

#[tokio::test]
async fn service_scope_comes_from_the_actual_jetstream_resolver() -> Result<(), Box<dyn std::error::Error>> {
    let broker = JetStreamTestServer::start_with_atomic_batches().await;
    let config = PeerStoreConfig::new(PeerTenant::new("configured")?, PeerStorageBudget::new(1_048_576)?);
    let store = PeerJetStreamStore::new(open_store(broker.jetstream().await, &config).await?)?;
    assert_eq!(store.tenant().as_str(), "configured");
    Ok(())
}
