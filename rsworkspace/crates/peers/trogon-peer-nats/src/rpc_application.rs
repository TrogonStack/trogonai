use crate::rpc_operation::PeerRpcOperation;
use grpc_nats_micro::ServiceFault;
use trogon_peer::TrustedPeerOwner;
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
    S: Send + Sync,
    P: Send + Sync,
    C: Send + Sync,
    Content: Send + Sync,
    Cache: Send + Sync,
    Ids: Send + Sync,
    Catalog: Send + Sync,
{
    type Response = proto::EnrollResponse;
    async fn execute(
        &self,
        _owner: &TrustedPeerOwner,
        _request: proto::EnrollRequest,
    ) -> Result<Self::Response, ServiceFault> {
        let _ = &self.service;
        Err(ServiceFault::internal("unimplemented"))
    }
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerRpcOperation<proto::HeartbeatRequest>
    for PeerRpcApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: Send + Sync,
    P: Send + Sync,
    C: Send + Sync,
    Content: Send + Sync,
    Cache: Send + Sync,
    Ids: Send + Sync,
    Catalog: Send + Sync,
{
    type Response = proto::HeartbeatResponse;
    async fn execute(
        &self,
        _owner: &TrustedPeerOwner,
        _request: proto::HeartbeatRequest,
    ) -> Result<Self::Response, ServiceFault> {
        let _ = &self.service;
        Err(ServiceFault::internal("unimplemented"))
    }
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerRpcOperation<proto::DetachRequest>
    for PeerRpcApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: Send + Sync,
    P: Send + Sync,
    C: Send + Sync,
    Content: Send + Sync,
    Cache: Send + Sync,
    Ids: Send + Sync,
    Catalog: Send + Sync,
{
    type Response = proto::DetachResponse;
    async fn execute(
        &self,
        _owner: &TrustedPeerOwner,
        _request: proto::DetachRequest,
    ) -> Result<Self::Response, ServiceFault> {
        let _ = &self.service;
        Err(ServiceFault::internal("unimplemented"))
    }
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerRpcOperation<proto::RosterRequest>
    for PeerRpcApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: Send + Sync,
    P: Send + Sync,
    C: Send + Sync,
    Content: Send + Sync,
    Cache: Send + Sync,
    Ids: Send + Sync,
    Catalog: Send + Sync,
{
    type Response = proto::RosterResponse;
    async fn execute(
        &self,
        _owner: &TrustedPeerOwner,
        _request: proto::RosterRequest,
    ) -> Result<Self::Response, ServiceFault> {
        let _ = &self.service;
        Err(ServiceFault::internal("unimplemented"))
    }
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerRpcOperation<proto::SendRequest>
    for PeerRpcApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: Send + Sync,
    P: Send + Sync,
    C: Send + Sync,
    Content: Send + Sync,
    Cache: Send + Sync,
    Ids: Send + Sync,
    Catalog: Send + Sync,
{
    type Response = proto::SendResponse;
    async fn execute(
        &self,
        _owner: &TrustedPeerOwner,
        _request: proto::SendRequest,
    ) -> Result<Self::Response, ServiceFault> {
        let _ = &self.service;
        Err(ServiceFault::internal("unimplemented"))
    }
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerRpcOperation<proto::StatusRequest>
    for PeerRpcApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: Send + Sync,
    P: Send + Sync,
    C: Send + Sync,
    Content: Send + Sync,
    Cache: Send + Sync,
    Ids: Send + Sync,
    Catalog: Send + Sync,
{
    type Response = proto::StatusResponse;
    async fn execute(
        &self,
        _owner: &TrustedPeerOwner,
        _request: proto::StatusRequest,
    ) -> Result<Self::Response, ServiceFault> {
        let _ = &self.service;
        Err(ServiceFault::internal("unimplemented"))
    }
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerRpcOperation<proto::PrepareBatchRequest>
    for PeerRpcApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: Send + Sync,
    P: Send + Sync,
    C: Send + Sync,
    Content: Send + Sync,
    Cache: Send + Sync,
    Ids: Send + Sync,
    Catalog: Send + Sync,
{
    type Response = proto::PrepareBatchResponse;
    async fn execute(
        &self,
        _owner: &TrustedPeerOwner,
        _request: proto::PrepareBatchRequest,
    ) -> Result<Self::Response, ServiceFault> {
        let _ = &self.service;
        Err(ServiceFault::internal("unimplemented"))
    }
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerRpcOperation<proto::AcknowledgeBatchRequest>
    for PeerRpcApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: Send + Sync,
    P: Send + Sync,
    C: Send + Sync,
    Content: Send + Sync,
    Cache: Send + Sync,
    Ids: Send + Sync,
    Catalog: Send + Sync,
{
    type Response = proto::AcknowledgeBatchResponse;
    async fn execute(
        &self,
        _owner: &TrustedPeerOwner,
        _request: proto::AcknowledgeBatchRequest,
    ) -> Result<Self::Response, ServiceFault> {
        let _ = &self.service;
        Err(ServiceFault::internal("unimplemented"))
    }
}
