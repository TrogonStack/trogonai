use crate::rpc_operation::PeerRpcOperation;
use grpc_nats_micro::ServiceFault;
use trogon_decider_runtime::{CommandAuthorizer, StreamAppend, StreamRead};
use trogon_peer::{PeerId, TrustedPeerOwner};
use trogon_peer_service::{
    BatchIdGenerator, ContentOpen, ContentSeal, MessageIdGenerator, PeerAccess, PeerCatalog, PeerClock,
    PresentationInsert, PresentationOrphan, PresentationRead, ScopedPeerStore,
};
use trogon_peer_service::{PeerApplication, PeerService};
use trogonai_proto::peers::v1alpha1 as proto;

pub struct PeerRpcApplication<S, P, C, Content, Cache, Ids, Catalog> {
    service: PeerService<S, P, C>,
    application: PeerApplication<S, P, C, Content, Cache, Ids, Catalog>,
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerRpcApplication<S, P, C, Content, Cache, Ids, Catalog> {
    pub fn new(
        service: PeerService<S, P, C>,
        application: PeerApplication<S, P, C, Content, Cache, Ids, Catalog>,
    ) -> Self {
        Self { service, application }
    }
    pub fn application(&self) -> &PeerApplication<S, P, C, Content, Cache, Ids, Catalog> {
        &self.application
    }
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerRpcOperation<proto::EnrollRequest>
    for PeerRpcApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: StreamRead<PeerId> + StreamAppend<PeerId> + ScopedPeerStore,
    P: Send + Sync + CommandAuthorizer<trogon_peer::Enroll> + CommandAuthorizer<trogon_peer::Attach>,
    C: PeerClock,
    Content: Send + Sync,
    Cache: Send + Sync,
    Ids: Send + Sync,
    Catalog: Send + Sync,
{
    type Response = proto::EnrollResponse;
    async fn execute(
        &self,
        owner: &TrustedPeerOwner,
        request: proto::EnrollRequest,
    ) -> Result<Self::Response, ServiceFault> {
        self.service
            .enroll(owner, request)
            .await
            .map_err(crate::rpc_fault::enrollment)
    }
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerRpcOperation<proto::HeartbeatRequest>
    for PeerRpcApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: StreamRead<PeerId> + StreamAppend<PeerId> + ScopedPeerStore,
    P: Send + Sync + CommandAuthorizer<trogon_peer::Heartbeat> + CommandAuthorizer<trogon_peer::Detach>,
    C: PeerClock,
    Content: Send + Sync,
    Cache: Send + Sync,
    Ids: Send + Sync,
    Catalog: Send + Sync,
{
    type Response = proto::HeartbeatResponse;
    async fn execute(
        &self,
        owner: &TrustedPeerOwner,
        request: proto::HeartbeatRequest,
    ) -> Result<Self::Response, ServiceFault> {
        self.service
            .heartbeat(owner, request)
            .await
            .map_err(crate::rpc_fault::application)
    }
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerRpcOperation<proto::DetachRequest>
    for PeerRpcApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: StreamRead<PeerId> + StreamAppend<PeerId> + ScopedPeerStore,
    P: Send + Sync + CommandAuthorizer<trogon_peer::Heartbeat> + CommandAuthorizer<trogon_peer::Detach>,
    C: PeerClock,
    Content: Send + Sync,
    Cache: Send + Sync,
    Ids: Send + Sync,
    Catalog: Send + Sync,
{
    type Response = proto::DetachResponse;
    async fn execute(
        &self,
        owner: &TrustedPeerOwner,
        request: proto::DetachRequest,
    ) -> Result<Self::Response, ServiceFault> {
        self.service
            .detach(owner, request)
            .await
            .map_err(crate::rpc_fault::application)
    }
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerRpcOperation<proto::RosterRequest>
    for PeerRpcApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: StreamRead<PeerId> + StreamAppend<PeerId> + ScopedPeerStore,
    P: Send + Sync + CommandAuthorizer<PeerAccess>,
    C: PeerClock,
    Content: Send + Sync,
    Cache: Send + Sync,
    Ids: Send + Sync,
    Catalog: Send + Sync,
    Catalog: PeerCatalog,
{
    type Response = proto::RosterResponse;
    async fn execute(
        &self,
        owner: &TrustedPeerOwner,
        request: proto::RosterRequest,
    ) -> Result<Self::Response, ServiceFault> {
        self.application
            .roster(owner, request)
            .await
            .map_err(crate::rpc_fault::application)
    }
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerRpcOperation<proto::SendRequest>
    for PeerRpcApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: StreamRead<PeerId> + StreamAppend<PeerId> + ScopedPeerStore,
    P: Send + Sync + CommandAuthorizer<trogon_peer::SendMessage> + CommandAuthorizer<PeerAccess>,
    C: PeerClock,
    Content: Send + Sync,
    Cache: Send + Sync,
    Ids: Send + Sync,
    Catalog: Send + Sync,
    Content: ContentSeal,
    Ids: MessageIdGenerator,
{
    type Response = proto::SendResponse;
    async fn execute(
        &self,
        owner: &TrustedPeerOwner,
        request: proto::SendRequest,
    ) -> Result<Self::Response, ServiceFault> {
        self.application
            .send(owner, request)
            .await
            .map_err(crate::rpc_fault::application)
    }
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerRpcOperation<proto::StatusRequest>
    for PeerRpcApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: StreamRead<PeerId> + StreamAppend<PeerId> + ScopedPeerStore,
    P: Send + Sync + CommandAuthorizer<trogon_peer::SendMessage> + CommandAuthorizer<PeerAccess>,
    C: PeerClock,
    Content: Send + Sync,
    Cache: Send + Sync,
    Ids: Send + Sync,
    Catalog: Send + Sync,
    Content: ContentSeal,
    Ids: MessageIdGenerator,
{
    type Response = proto::StatusResponse;
    async fn execute(
        &self,
        owner: &TrustedPeerOwner,
        request: proto::StatusRequest,
    ) -> Result<Self::Response, ServiceFault> {
        self.application
            .status(owner, request)
            .await
            .map_err(crate::rpc_fault::application)
    }
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerRpcOperation<proto::PrepareBatchRequest>
    for PeerRpcApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: StreamRead<PeerId> + StreamAppend<PeerId> + ScopedPeerStore,
    P: Send + Sync + CommandAuthorizer<trogon_peer::PrepareBatch> + CommandAuthorizer<PeerAccess>,
    C: PeerClock,
    Content: Send + Sync,
    Cache: Send + Sync,
    Ids: Send + Sync,
    Catalog: Send + Sync,
    Content: ContentOpen,
    Cache: PresentationRead + PresentationInsert + PresentationOrphan,
    Ids: BatchIdGenerator,
{
    type Response = proto::PrepareBatchResponse;
    async fn execute(
        &self,
        owner: &TrustedPeerOwner,
        request: proto::PrepareBatchRequest,
    ) -> Result<Self::Response, ServiceFault> {
        self.application
            .prepare_batch(owner, request)
            .await
            .map_err(crate::rpc_fault::application)
    }
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerRpcOperation<proto::AcknowledgeBatchRequest>
    for PeerRpcApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: StreamRead<PeerId> + StreamAppend<PeerId> + ScopedPeerStore,
    P: Send + Sync + CommandAuthorizer<trogon_peer::AcknowledgeBatch> + CommandAuthorizer<PeerAccess>,
    C: PeerClock,
    Content: Send + Sync,
    Cache: Send + Sync,
    Ids: Send + Sync,
    Catalog: Send + Sync,
{
    type Response = proto::AcknowledgeBatchResponse;
    async fn execute(
        &self,
        owner: &TrustedPeerOwner,
        request: proto::AcknowledgeBatchRequest,
    ) -> Result<Self::Response, ServiceFault> {
        self.application
            .acknowledge_batch(owner, request)
            .await
            .map_err(crate::rpc_fault::application)
    }
}
