use crate::telemetry::FaultOrigin;
use buffa::Enumeration as _;
use grpc_nats_micro::{ServiceErrorCode, ServiceFault};
use trogon_decider_runtime::CommandError;
use trogon_peer_service::{EnrollServiceError, ExecutionError, HistoryError, PeerApplicationError, PeerPortError};
use trogonai_proto::google::rpc::{Code, Status};

fn fault(code: Code, message: &'static str) -> ServiceFault {
    ServiceFault::with_code(
        ServiceErrorCode::new(code.to_i32()).unwrap_or(ServiceErrorCode::INTERNAL),
        Status {
            message: message.into(),
            ..Default::default()
        },
    )
}
fn observed_fault(origin: FaultOrigin, code: Code, message: &'static str) -> ServiceFault {
    crate::telemetry::record_fault(origin, code);
    fault(code, message)
}
fn history<R>(error: HistoryError<R>) -> ServiceFault {
    match error {
        HistoryError::Read(_) => {
            observed_fault(FaultOrigin::HistoryRead, Code::UNAVAILABLE, "peer storage unavailable")
        }
        HistoryError::Limit => fault(Code::RESOURCE_EXHAUSTED, "peer history limit reached"),
        HistoryError::Decode(_) | HistoryError::UnknownEvent | HistoryError::Evolve(_) | HistoryError::Incomplete => {
            observed_fault(FaultOrigin::HistoryIntegrity, Code::INTERNAL, "peer history invalid")
        }
    }
}
fn execution<D, R, A>(error: ExecutionError<D, R, A>) -> ServiceFault {
    match error {
        CommandError::Unauthorized(_) => fault(Code::PERMISSION_DENIED, "peer operation denied"),
        CommandError::Decide(_) => fault(Code::FAILED_PRECONDITION, "peer precondition failed"),
        CommandError::ReadStream(_) => observed_fault(
            FaultOrigin::ExecutionReadStream,
            Code::UNAVAILABLE,
            "peer storage unavailable",
        ),
        CommandError::ReadSnapshot(source) => match source {},
        CommandError::Append(_) => observed_fault(
            FaultOrigin::ExecutionAppend,
            Code::UNAVAILABLE,
            "peer storage unavailable",
        ),
        CommandError::Overloaded(_) | CommandError::ReplayLimitExceeded(_) => {
            fault(Code::RESOURCE_EXHAUSTED, "peer capacity exhausted")
        }
        CommandError::PreconditionConflict(_) => fault(Code::ABORTED, "peer revision conflict"),
        CommandError::Evolve(_)
        | CommandError::EventType(_)
        | CommandError::EventEncode(_)
        | CommandError::DecodeEvent(_)
        | CommandError::SnapshotAheadOfStream(_)
        | CommandError::ReadAfterOverflow(_) => {
            observed_fault(FaultOrigin::ExecutionIntegrity, Code::INTERNAL, "peer execution failed")
        }
    }
}
pub(crate) fn application<R, A>(error: PeerApplicationError<R, A>) -> ServiceFault {
    match error {
        PeerApplicationError::Input | PeerApplicationError::Value(_) | PeerApplicationError::Observation(_) => {
            fault(Code::INVALID_ARGUMENT, "invalid peer request")
        }
        PeerApplicationError::Scope | PeerApplicationError::Denied(_) => {
            fault(Code::PERMISSION_DENIED, "peer operation denied")
        }
        PeerApplicationError::History(error) => history(error),
        PeerApplicationError::LifetimeHistory(error) => match error {
            HistoryError::Read(_) => {
                observed_fault(FaultOrigin::HistoryRead, Code::UNAVAILABLE, "peer storage unavailable")
            }
            HistoryError::Limit => fault(Code::RESOURCE_EXHAUSTED, "peer capacity exhausted"),
            HistoryError::Decode(_)
            | HistoryError::UnknownEvent
            | HistoryError::Evolve(_)
            | HistoryError::Incomplete => {
                observed_fault(FaultOrigin::HistoryIntegrity, Code::INTERNAL, "peer execution failed")
            }
        },
        PeerApplicationError::Heartbeat(error) => execution(error),
        PeerApplicationError::Detach(error) => execution(error),
        PeerApplicationError::Message(error) => execution(error),
        PeerApplicationError::Port(error) => match error {
            PeerPortError::Denied => fault(Code::PERMISSION_DENIED, "peer operation denied"),
            PeerPortError::Conflict => fault(Code::ABORTED, "peer record conflict"),
            PeerPortError::Unavailable => fault(Code::UNAVAILABLE, "peer port unavailable"),
            PeerPortError::Backend(_) => {
                observed_fault(FaultOrigin::PortBackend, Code::UNAVAILABLE, "peer port unavailable")
            }
            PeerPortError::Integrity => fault(Code::INTERNAL, "peer integrity check failed"),
        },
        PeerApplicationError::Evidence(_) => fault(Code::FAILED_PRECONDITION, "peer precondition failed"),
        PeerApplicationError::Context(_) => {
            observed_fault(FaultOrigin::Context, Code::INTERNAL, "peer execution failed")
        }
        PeerApplicationError::Principal(_) => fault(Code::INTERNAL, "peer execution failed"),
    }
}
pub(crate) fn enrollment<R, A>(error: EnrollServiceError<R, A>) -> ServiceFault {
    match error {
        EnrollServiceError::Scope => fault(Code::PERMISSION_DENIED, "peer operation denied"),
        EnrollServiceError::MissingInput { .. } | EnrollServiceError::InvalidInput { .. } => {
            fault(Code::INVALID_ARGUMENT, "invalid peer request")
        }
        EnrollServiceError::Execution(error) => execution(error),
        EnrollServiceError::Attach(error) => execution(error),
        EnrollServiceError::History(error) => history(error),
        EnrollServiceError::ResumeGuard(_) => fault(Code::FAILED_PRECONDITION, "peer attachment changed"),
        EnrollServiceError::Principal(_) | EnrollServiceError::ResumeReference(_) => {
            fault(Code::INTERNAL, "peer execution failed")
        }
    }
}
