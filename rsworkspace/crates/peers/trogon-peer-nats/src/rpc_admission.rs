use crate::{PeerIngress, PeerMethod};
use grpc_nats_micro::{EndpointAdmission, EndpointHandler, RequestMetadata};
use std::sync::Arc;

pub struct PeerRpcAdmission<B> {
    pub ingress: PeerIngress,
    pub method: PeerMethod,
    pub backend: Arc<B>,
}
impl<B: Send + Sync> EndpointAdmission for PeerRpcAdmission<B> {
    fn admit(&self, _: RequestMetadata<'_>) -> Option<Box<dyn EndpointHandler + '_>> {
        None
    }
}
