use crate::{
    peer_ingress::PeerIngress, peer_method::PeerMethod, peer_reply_subject::PeerReplySubject,
    rpc_operation::PeerRpcOperation,
};
use grpc_nats_micro::{ContentType, EndpointAdmission, EndpointHandler, RequestMetadata, ServiceFault};
use std::{future::Future, pin::Pin, sync::Arc};
use trogon_peer::{CallerId, TenantId, TrustedPeerOwner};
use trogonai_proto::peers::v1alpha1 as proto;

pub struct PeerRpcAdmission<B> {
    pub ingress: PeerIngress,
    pub method: PeerMethod,
    pub backend: Arc<B>,
}
impl<B> EndpointAdmission for PeerRpcAdmission<B>
where
    B: PeerRpcOperation<proto::EnrollRequest, Response = proto::EnrollResponse>
        + PeerRpcOperation<proto::HeartbeatRequest, Response = proto::HeartbeatResponse>
        + PeerRpcOperation<proto::DetachRequest, Response = proto::DetachResponse>
        + PeerRpcOperation<proto::RosterRequest, Response = proto::RosterResponse>
        + PeerRpcOperation<proto::SendRequest, Response = proto::SendResponse>
        + PeerRpcOperation<proto::StatusRequest, Response = proto::StatusResponse>
        + PeerRpcOperation<proto::PrepareBatchRequest, Response = proto::PrepareBatchResponse>
        + PeerRpcOperation<proto::AcknowledgeBatchRequest, Response = proto::AcknowledgeBatchResponse>,
{
    fn admit(&self, metadata: RequestMetadata<'_>) -> Option<Box<dyn EndpointHandler + '_>> {
        let route = self.ingress.parse(metadata.subject).ok()?;
        if route.method() != self.method {
            return None;
        }
        PeerReplySubject::parse_for(route.principal(), metadata.reply?).ok()?;
        let owner = TrustedPeerOwner::new(
            TenantId::new(route.principal().tenant().as_str()).ok()?,
            CallerId::new(route.principal().caller().as_str()).ok()?,
        );
        Some(Box::new(AuthorizedHandler {
            backend: self.backend.as_ref(),
            owner,
            method: route.method(),
        }))
    }
}
struct AuthorizedHandler<'a, B> {
    backend: &'a B,
    owner: TrustedPeerOwner,
    method: PeerMethod,
}
impl<B> EndpointHandler for AuthorizedHandler<'_, B>
where
    B: PeerRpcOperation<proto::EnrollRequest, Response = proto::EnrollResponse>
        + PeerRpcOperation<proto::HeartbeatRequest, Response = proto::HeartbeatResponse>
        + PeerRpcOperation<proto::DetachRequest, Response = proto::DetachResponse>
        + PeerRpcOperation<proto::RosterRequest, Response = proto::RosterResponse>
        + PeerRpcOperation<proto::SendRequest, Response = proto::SendResponse>
        + PeerRpcOperation<proto::StatusRequest, Response = proto::StatusResponse>
        + PeerRpcOperation<proto::PrepareBatchRequest, Response = proto::PrepareBatchResponse>
        + PeerRpcOperation<proto::AcknowledgeBatchRequest, Response = proto::AcknowledgeBatchResponse>,
{
    fn handle<'a>(
        &'a self,
        bytes: &'a [u8],
        encoding: ContentType,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ServiceFault>> + Send + 'a>> {
        Box::pin(async move {
            match self.method {
                PeerMethod::Enroll => {
                    execute::<proto::EnrollRequest, _>(self.backend, &self.owner, bytes, encoding).await
                }
                PeerMethod::Heartbeat => {
                    execute::<proto::HeartbeatRequest, _>(self.backend, &self.owner, bytes, encoding).await
                }
                PeerMethod::Detach => {
                    execute::<proto::DetachRequest, _>(self.backend, &self.owner, bytes, encoding).await
                }
                PeerMethod::Roster => {
                    execute::<proto::RosterRequest, _>(self.backend, &self.owner, bytes, encoding).await
                }
                PeerMethod::Send => execute::<proto::SendRequest, _>(self.backend, &self.owner, bytes, encoding).await,
                PeerMethod::Status => {
                    execute::<proto::StatusRequest, _>(self.backend, &self.owner, bytes, encoding).await
                }
                PeerMethod::PrepareBatch => {
                    execute::<proto::PrepareBatchRequest, _>(self.backend, &self.owner, bytes, encoding).await
                }
                PeerMethod::AcknowledgeBatch => {
                    execute::<proto::AcknowledgeBatchRequest, _>(self.backend, &self.owner, bytes, encoding).await
                }
            }
        })
    }
}
async fn execute<Request, B>(
    backend: &B,
    owner: &TrustedPeerOwner,
    bytes: &[u8],
    encoding: ContentType,
) -> Result<Vec<u8>, ServiceFault>
where
    Request: buffa::Message + serde::de::DeserializeOwned,
    B: PeerRpcOperation<Request>,
{
    let request = encoding
        .decode(bytes)
        .map_err(|_| ServiceFault::invalid_argument("invalid peer request"))?;
    let response = backend.execute(owner, request).await?;
    encoding
        .encode(&response)
        .map_err(|_| ServiceFault::internal("peer response encoding failed"))
}
