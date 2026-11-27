mod fault_origin;

use buffa::Enumeration as _;
use trogonai_proto::google::rpc::Code;

pub(crate) use fault_origin::FaultOrigin;

pub(crate) fn record_fault(origin: FaultOrigin, code: Code) {
    tracing::warn!(
        target: "trogon_peer_nats::telemetry",
        { "error.type" = origin.as_str(), "rpc.grpc.status_code" = code.to_i32() },
        "peer RPC fault"
    );
}
