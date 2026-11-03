use crate::{peer_requester::PeerRequester, rpc_client_error::PeerRpcClientError};
use grpc_nats_micro::ContentType;
use trogonai_proto::peers::v1alpha1 as proto;
#[derive(Clone)]
pub struct PeerRpcClient {
    requester: PeerRequester,
    encoding: ContentType,
}
impl PeerRpcClient {
    pub fn new(requester: PeerRequester, encoding: ContentType) -> Self {
        Self { requester, encoding }
    }
    pub async fn enroll(&self, _request: &proto::EnrollRequest) -> Result<proto::EnrollResponse, PeerRpcClientError> {
        let _ = (&self.requester, self.encoding);
        Err(PeerRpcClientError::Unimplemented)
    }
    pub async fn heartbeat(
        &self,
        _request: &proto::HeartbeatRequest,
    ) -> Result<proto::HeartbeatResponse, PeerRpcClientError> {
        let _ = (&self.requester, self.encoding);
        Err(PeerRpcClientError::Unimplemented)
    }
    pub async fn detach(&self, _request: &proto::DetachRequest) -> Result<proto::DetachResponse, PeerRpcClientError> {
        let _ = (&self.requester, self.encoding);
        Err(PeerRpcClientError::Unimplemented)
    }
    pub async fn roster(&self, _request: &proto::RosterRequest) -> Result<proto::RosterResponse, PeerRpcClientError> {
        let _ = (&self.requester, self.encoding);
        Err(PeerRpcClientError::Unimplemented)
    }
    pub async fn send(&self, _request: &proto::SendRequest) -> Result<proto::SendResponse, PeerRpcClientError> {
        let _ = (&self.requester, self.encoding);
        Err(PeerRpcClientError::Unimplemented)
    }
    pub async fn status(&self, _request: &proto::StatusRequest) -> Result<proto::StatusResponse, PeerRpcClientError> {
        let _ = (&self.requester, self.encoding);
        Err(PeerRpcClientError::Unimplemented)
    }
    pub async fn prepare_batch(
        &self,
        _request: &proto::PrepareBatchRequest,
    ) -> Result<proto::PrepareBatchResponse, PeerRpcClientError> {
        let _ = (&self.requester, self.encoding);
        Err(PeerRpcClientError::Unimplemented)
    }
    pub async fn acknowledge_batch(
        &self,
        _request: &proto::AcknowledgeBatchRequest,
    ) -> Result<proto::AcknowledgeBatchResponse, PeerRpcClientError> {
        let _ = (&self.requester, self.encoding);
        Err(PeerRpcClientError::Unimplemented)
    }
}
