use std::error::Error;
use std::future::Future;

use trogonai_proto::peers::v1alpha1::{
    AcknowledgeBatchRequest, AcknowledgeBatchResponse, EnrollRequest, EnrollResponse, PrepareBatchRequest,
    PrepareBatchResponse, RosterRequest, RosterResponse, SendRequest, SendResponse, StatusRequest, StatusResponse,
};

pub trait EnrollPeer: Send + Sync + 'static {
    type Error: Error + Send + Sync + 'static;

    fn enroll(&self, request: EnrollRequest) -> impl Future<Output = Result<EnrollResponse, Self::Error>> + Send;
}

pub trait PreparePeerBatch: Send + Sync + 'static {
    type Error: Error + Send + Sync + 'static;

    fn prepare_batch(
        &self,
        request: PrepareBatchRequest,
    ) -> impl Future<Output = Result<PrepareBatchResponse, Self::Error>> + Send;
}

pub trait ReadPeerRoster: Send + Sync + 'static {
    type Error: Error + Send + Sync + 'static;

    fn roster(&self, request: RosterRequest) -> impl Future<Output = Result<RosterResponse, Self::Error>> + Send;
}

pub trait SendPeerMessage: Send + Sync + 'static {
    type Error: Error + Send + Sync + 'static;

    fn send(&self, request: SendRequest) -> impl Future<Output = Result<SendResponse, Self::Error>> + Send;
}

pub trait ReadPeerMessageStatus: Send + Sync + 'static {
    type Error: Error + Send + Sync + 'static;

    fn status(&self, request: StatusRequest) -> impl Future<Output = Result<StatusResponse, Self::Error>> + Send;
}

pub trait AcknowledgePeerBatch: Send + Sync + 'static {
    type Error: Error + Send + Sync + 'static;

    fn acknowledge_batch(
        &self,
        request: AcknowledgeBatchRequest,
    ) -> impl Future<Output = Result<AcknowledgeBatchResponse, Self::Error>> + Send;
}

pub trait HeartbeatPeer: Send + Sync + 'static {
    type Error: Error + Send + Sync + 'static;

    fn heartbeat(
        &self,
        request: trogonai_proto::peers::v1alpha1::HeartbeatRequest,
    ) -> impl Future<Output = Result<trogonai_proto::peers::v1alpha1::HeartbeatResponse, Self::Error>> + Send;
}

pub trait DetachPeer: Send + Sync + 'static {
    type Error: Error + Send + Sync + 'static;

    fn detach(
        &self,
        request: trogonai_proto::peers::v1alpha1::DetachRequest,
    ) -> impl Future<Output = Result<trogonai_proto::peers::v1alpha1::DetachResponse, Self::Error>> + Send;
}
