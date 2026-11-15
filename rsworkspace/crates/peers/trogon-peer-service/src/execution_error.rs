use std::convert::Infallible;
use trogon_decider_runtime::CommandError;
use trogon_peer::EvolveError;
use trogonai_proto::peers::PeerEventPayloadError;

pub type ExecutionError<DecisionError, ReadError, AppendError> = CommandError<
    DecisionError,
    EvolveError,
    Infallible,
    ReadError,
    AppendError,
    PeerEventPayloadError,
    PeerEventPayloadError,
    PeerEventPayloadError,
>;
