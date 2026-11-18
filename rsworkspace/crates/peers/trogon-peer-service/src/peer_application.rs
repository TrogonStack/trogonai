use crate::peer_application_error::PeerApplicationError;
use trogon_decider_runtime::{StreamAppend, StreamRead};
use trogon_peer::{PeerId, TrustedPeerOwner};
use trogonai_proto::peers::v1alpha1 as proto;
pub type ApplicationResult<T, S> =
    Result<T, PeerApplicationError<<S as StreamRead<PeerId>>::Error, <S as StreamAppend<PeerId>>::Error>>;
pub struct PeerApplication<S, P, C, Content, Cache, Ids, Catalog> {
    pub store: S,
    pub policy: P,
    pub clock: C,
    pub content: Content,
    pub cache: Cache,
    pub ids: Ids,
    pub catalog: Catalog,
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerApplication<S, P, C, Content, Cache, Ids, Catalog> {
    pub fn new(store: S, policy: P, clock: C, content: Content, cache: Cache, ids: Ids, catalog: Catalog) -> Self {
        Self {
            store,
            policy,
            clock,
            content,
            cache,
            ids,
            catalog,
        }
    }
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: StreamRead<PeerId> + StreamAppend<PeerId>,
{
    pub async fn prepare_batch(
        &self,
        _owner: &TrustedPeerOwner,
        _request: proto::PrepareBatchRequest,
    ) -> ApplicationResult<proto::PrepareBatchResponse, S> {
        Err(PeerApplicationError::NotImplemented)
    }
    pub async fn acknowledge_batch(
        &self,
        _owner: &TrustedPeerOwner,
        _request: proto::AcknowledgeBatchRequest,
    ) -> ApplicationResult<proto::AcknowledgeBatchResponse, S> {
        Err(PeerApplicationError::NotImplemented)
    }
    pub async fn roster(
        &self,
        _owner: &TrustedPeerOwner,
        _request: proto::RosterRequest,
    ) -> ApplicationResult<proto::RosterResponse, S> {
        Err(PeerApplicationError::NotImplemented)
    }
}
