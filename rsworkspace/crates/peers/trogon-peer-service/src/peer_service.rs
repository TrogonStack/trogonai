use trogon_decider_runtime::{CommandAuthorizer, StreamAppend, StreamRead};
use trogon_peer::{Enroll, PeerId, TrustedPeerOwner};
use trogonai_proto::peers::v1alpha1;

use crate::{EnrollResult, EnrollServiceError, PeerClock};

pub struct PeerService<Store, Authorizer, Clock> {
    _store: Store,
    _authorizer: Authorizer,
    _clock: Clock,
}

impl<Store, Authorizer, Clock> PeerService<Store, Authorizer, Clock>
where
    Store: StreamRead<PeerId> + StreamAppend<PeerId>,
    Authorizer: CommandAuthorizer<Enroll> + Send + Sync,
    Clock: PeerClock,
{
    pub fn new(store: Store, authorizer: Authorizer, clock: Clock) -> Self {
        Self { _store: store, _authorizer: authorizer, _clock: clock }
    }

    pub async fn enroll(
        &self,
        _owner: &TrustedPeerOwner,
        _request: v1alpha1::EnrollRequest,
    ) -> EnrollResult<<Store as StreamRead<PeerId>>::Error, <Store as StreamAppend<PeerId>>::Error> {
        Err(EnrollServiceError::NotImplemented)
    }
}
