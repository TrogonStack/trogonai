use grpc_nats_micro::ServiceFault;
use std::future::Future;
use trogon_peer::TrustedPeerOwner;

pub trait PeerRpcOperation<Request>: Send + Sync {
    type Response: buffa::Message + serde::Serialize + Send;
    fn execute(
        &self,
        owner: &TrustedPeerOwner,
        request: Request,
    ) -> impl Future<Output = Result<Self::Response, ServiceFault>> + Send;
}
