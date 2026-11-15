use std::sync::Arc;
use trogon_peer_nats::{PeerRpcApplication, peer::PeerJetStreamStore};
use trogon_peer_service::{
    PeerApplication, PeerService,
    test_support::{
        CatalogFixture, ClockFixture, DisposableContentFixture, Fixture, IdFixture, PolicyFixture, PresentationFixture,
    },
};

pub type Application = PeerRpcApplication<
    PeerJetStreamStore,
    PolicyFixture,
    ClockFixture,
    DisposableContentFixture,
    PresentationFixture,
    IdFixture,
    CatalogFixture,
>;
pub fn compose(store: PeerJetStreamStore, fixture: &Fixture) -> Arc<Application> {
    Arc::new(PeerRpcApplication::new(
        PeerService::new(store.clone(), fixture.policy.clone(), fixture.clock.clone()),
        PeerApplication::new(
            store,
            fixture.policy.clone(),
            fixture.clock.clone(),
            fixture.content.clone(),
            fixture.cache.clone(),
            fixture.ids.clone(),
            fixture.catalog.clone(),
        ),
    ))
}
