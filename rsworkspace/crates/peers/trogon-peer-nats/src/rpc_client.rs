use crate::{peer_method::PeerMethod, peer_requester::PeerRequester, rpc_client_error::PeerRpcClientError};
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
    pub async fn enroll(&self, request: &proto::EnrollRequest) -> Result<proto::EnrollResponse, PeerRpcClientError> {
        self.call(PeerMethod::Enroll, request).await
    }
    pub async fn heartbeat(
        &self,
        request: &proto::HeartbeatRequest,
    ) -> Result<proto::HeartbeatResponse, PeerRpcClientError> {
        self.call(PeerMethod::Heartbeat, request).await
    }
    pub async fn detach(&self, request: &proto::DetachRequest) -> Result<proto::DetachResponse, PeerRpcClientError> {
        self.call(PeerMethod::Detach, request).await
    }
    pub async fn roster(&self, request: &proto::RosterRequest) -> Result<proto::RosterResponse, PeerRpcClientError> {
        self.call(PeerMethod::Roster, request).await
    }
    pub async fn send(&self, request: &proto::SendRequest) -> Result<proto::SendResponse, PeerRpcClientError> {
        self.call(PeerMethod::Send, request).await
    }
    pub async fn status(&self, request: &proto::StatusRequest) -> Result<proto::StatusResponse, PeerRpcClientError> {
        self.call(PeerMethod::Status, request).await
    }
    pub async fn prepare_batch(
        &self,
        request: &proto::PrepareBatchRequest,
    ) -> Result<proto::PrepareBatchResponse, PeerRpcClientError> {
        self.call(PeerMethod::PrepareBatch, request).await
    }
    pub async fn acknowledge_batch(
        &self,
        request: &proto::AcknowledgeBatchRequest,
    ) -> Result<proto::AcknowledgeBatchResponse, PeerRpcClientError> {
        self.call(PeerMethod::AcknowledgeBatch, request).await
    }

    async fn call<Request, Response>(
        &self,
        method: PeerMethod,
        request: &Request,
    ) -> Result<Response, PeerRpcClientError>
    where
        Request: buffa::Message + serde::Serialize,
        Response: buffa::Message + serde::de::DeserializeOwned,
    {
        let payload = self.encoding.encode(request)?;
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(
            grpc_nats_micro::constants::HEADER_CONTENT_TYPE,
            self.encoding.header_value(),
        );
        let response = self.requester.request_with_headers(method, headers, payload).await?;
        grpc_nats_micro::status_codec::decode_reply(response.headers.as_ref(), &response.payload, self.encoding)
            .map_err(PeerRpcClientError::from)
    }
}
