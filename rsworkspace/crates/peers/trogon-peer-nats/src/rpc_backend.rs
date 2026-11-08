use crate::rpc_operation::PeerRpcOperation;
use trogonai_proto::peers::v1alpha1 as proto;

pub trait PeerRpcBackend:
    PeerRpcOperation<proto::EnrollRequest, Response = proto::EnrollResponse>
    + PeerRpcOperation<proto::HeartbeatRequest, Response = proto::HeartbeatResponse>
    + PeerRpcOperation<proto::DetachRequest, Response = proto::DetachResponse>
    + PeerRpcOperation<proto::RosterRequest, Response = proto::RosterResponse>
    + PeerRpcOperation<proto::SendRequest, Response = proto::SendResponse>
    + PeerRpcOperation<proto::StatusRequest, Response = proto::StatusResponse>
    + PeerRpcOperation<proto::PrepareBatchRequest, Response = proto::PrepareBatchResponse>
    + PeerRpcOperation<proto::AcknowledgeBatchRequest, Response = proto::AcknowledgeBatchResponse>
{
}
impl<B> PeerRpcBackend for B where
    B: PeerRpcOperation<proto::EnrollRequest, Response = proto::EnrollResponse>
        + PeerRpcOperation<proto::HeartbeatRequest, Response = proto::HeartbeatResponse>
        + PeerRpcOperation<proto::DetachRequest, Response = proto::DetachResponse>
        + PeerRpcOperation<proto::RosterRequest, Response = proto::RosterResponse>
        + PeerRpcOperation<proto::SendRequest, Response = proto::SendResponse>
        + PeerRpcOperation<proto::StatusRequest, Response = proto::StatusResponse>
        + PeerRpcOperation<proto::PrepareBatchRequest, Response = proto::PrepareBatchResponse>
        + PeerRpcOperation<proto::AcknowledgeBatchRequest, Response = proto::AcknowledgeBatchResponse>
{
}
