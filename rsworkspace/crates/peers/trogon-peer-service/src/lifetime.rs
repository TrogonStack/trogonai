use crate::{
    peer_access::PeerAccess, peer_application_error::PeerApplicationError, peer_clock::PeerClock,
    peer_service::PeerService,
};
use trogon_decider_runtime::{CommandAuthorizer, StreamAppend, StreamRead};
use trogon_peer::{PeerId, TrustedPeerOwner};
use trogonai_proto::peers::v1alpha1 as proto;
impl<S, P, C> PeerService<S, P, C>
where
    S: StreamRead<PeerId> + StreamAppend<PeerId>,
    P: CommandAuthorizer<PeerAccess>,
    C: PeerClock,
{
    pub async fn heartbeat(
        &self,
        _owner: &TrustedPeerOwner,
        _request: proto::HeartbeatRequest,
    ) -> Result<
        proto::HeartbeatResponse,
        PeerApplicationError<<S as StreamRead<PeerId>>::Error, <S as StreamAppend<PeerId>>::Error>,
    > {
        Err(PeerApplicationError::NotImplemented)
    }
    pub async fn detach(
        &self,
        _owner: &TrustedPeerOwner,
        _request: proto::DetachRequest,
    ) -> Result<
        proto::DetachResponse,
        PeerApplicationError<<S as StreamRead<PeerId>>::Error, <S as StreamAppend<PeerId>>::Error>,
    > {
        Err(PeerApplicationError::NotImplemented)
    }
}
