//! Projecting one execution onto the ADR#0057 reply.
//!
//! Which of the two replies ADR#0016 defines a command earns is decided here,
//! and only here. A command that ran to completion answers with the `Decide`
//! method's response message; a command the host could not execute at all
//! answers with one complete `google.rpc.Status` under
//! `Nats-Service-Error-Code`. Deriving those two independently is what would
//! let a reply carry an error header over a success body, so both come from
//! one value.
//!
//! This module decides which of the two an error belongs in. What its
//! `google.rpc.Status` looks like belongs to [`crate::status`].

use buffa::{Message as _, MessageField};
use buffa_types::google::protobuf::Any;
use std::error::Error as StdError;
use trogon_decider_nats::OptimisticConcurrencyConflictError;
use trogon_decider_runtime::Event;
use trogon_decider_wasm_runtime::{GuestDomainError, ModuleName, WasmCommandError, WasmExecutionResult};
use trogon_semconv::attribute::DecisionOutcome;
use trogonai_proto::decider::{DecideResponseCase, v1};
use trogonai_proto::google::rpc::Status;

use crate::constants::TYPE_URL_PREFIX;
use crate::status::{self, FaultClass};

/// One command's outcome, as the reply that carries it.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandReply {
    body: ReplyBody,
    decision: DecisionOutcome,
}

/// The two reply shapes ADR#0016 defines, and nothing between them.
#[derive(Debug, Clone, PartialEq)]
enum ReplyBody {
    /// The execution ran to completion, so the body is the method's response.
    Response(v1::DecideResponse),
    /// The host could not execute the command, so the body is one `Status`.
    ServiceError(Status),
}

impl CommandReply {
    /// The command was decided and its events appended.
    pub fn decided(result: &WasmExecutionResult) -> Self {
        Self::from_case(
            DecisionOutcome::Decided,
            DecideResponseCase::Accepted(Box::new(v1::CommandAccepted {
                stream_position: result.stream_position.as_u64(),
                events: result.events.iter().map(decided_event).collect(),
            })),
        )
    }

    /// The module answered no.
    ///
    /// A response rather than a service error: the host executed correctly and
    /// the domain refused, and counting that in micro's `num_errors` would turn
    /// a health signal into a business-outcome counter.
    pub fn rejected(module: &ModuleName, error: &GuestDomainError) -> Self {
        Self::from_case(
            DecisionOutcome::Rejected,
            DecideResponseCase::Rejected(Box::new(status::rejected(
                module.as_str(),
                &error.code,
                error.message.clone(),
                &error.details,
            ))),
        )
    }

    /// No activated module claims the command type the subject named.
    pub fn unroutable(error: &dyn StdError) -> Self {
        Self::faulted(FaultClass::Unroutable, error)
    }

    /// The subject or the headers could not be read as a command at all.
    pub fn invalid_request(error: &dyn StdError) -> Self {
        Self::faulted(FaultClass::InvalidRequest, error)
    }

    /// The host broke one of its own invariants, so neither the guest nor
    /// storage is answerable for the failure.
    pub fn internal(error: &dyn StdError) -> Self {
        Self::faulted(FaultClass::Internal, error)
    }

    /// Maps the runtime's error taxonomy onto the reply, per ADR#0057 section 5.
    ///
    /// The module is named because a rejection's code space belongs to it: the
    /// host reports that code under the module's domain rather than under its
    /// own, so two modules choosing the same code stay distinguishable.
    pub fn from_command_error<ReadSnapshotError, ReadStreamError, AppendStreamError>(
        module: &ModuleName,
        error: &WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>,
    ) -> Self
    where
        ReadSnapshotError: StdError + 'static,
        ReadStreamError: StdError + 'static,
        AppendStreamError: StdError + 'static,
    {
        match error {
            WasmCommandError::Rejected(domain) => Self::rejected(module, domain),
            WasmCommandError::Overloaded(overloaded) => {
                Self::service_error(DecisionOutcome::Shed, status::shed(*overloaded))
            }
            WasmCommandError::Unauthorized(unauthorized) => {
                Self::service_error(DecisionOutcome::Denied, status::denied(unauthorized))
            }
            WasmCommandError::PreconditionConflict(_) => Self::faulted(FaultClass::UnsatisfiablePrecondition, error),
            // A concurrent writer won the race. Reaching for it through the
            // append error's chain rather than the variant, because the store
            // reports it as one storage failure among many and only the cause
            // tells a retryable conflict apart from a broken JetStream.
            WasmCommandError::Append(append) if carries_write_conflict(append) => {
                Self::faulted(FaultClass::Conflict, error)
            }
            WasmCommandError::Faulted(domain)
            | WasmCommandError::Evolve(domain)
            | WasmCommandError::StreamId(domain) => Self::service_error(
                DecisionOutcome::Faulted,
                status::guest_faulted(error.to_string(), &domain.details),
            ),
            WasmCommandError::Trap(_) | WasmCommandError::EmptyDecision | WasmCommandError::Instantiate(_) => {
                Self::faulted(FaultClass::Guest, error)
            }
            WasmCommandError::DeadlineExceeded(_) => Self::faulted(FaultClass::DeadlineExceeded, error),
            WasmCommandError::ReadSnapshot(_) | WasmCommandError::ReadStream(_) | WasmCommandError::Append(_) => {
                Self::faulted(FaultClass::Storage, error)
            }
            WasmCommandError::ReplayLimitExceeded(_)
            | WasmCommandError::SnapshotAheadOfStream(_)
            | WasmCommandError::ReadAfterOverflow(_)
            | WasmCommandError::Blocking(_) => Self::faulted(FaultClass::Internal, error),
        }
    }

    /// The discriminant this reply reports, shared with the decider's telemetry.
    ///
    /// Telemetry only. What a caller reads is the presence of the micro error
    /// header, per ADR#0016, and never a discriminant the host also publishes.
    pub const fn decision(&self) -> DecisionOutcome {
        self.decision
    }

    /// The `Status` that makes this reply a service error, if it is one.
    ///
    /// `Some` is exactly the condition for setting `Nats-Service-Error-Code`,
    /// so the header and the body cannot disagree about which reply this is.
    pub const fn service_error_status(&self) -> Option<&Status> {
        match &self.body {
            ReplyBody::ServiceError(status) => Some(status),
            ReplyBody::Response(_) => None,
        }
    }

    /// The method's response, if the execution ran to completion.
    pub const fn response(&self) -> Option<&v1::DecideResponse> {
        match &self.body {
            ReplyBody::Response(response) => Some(response),
            ReplyBody::ServiceError(_) => None,
        }
    }

    /// Encodes the reply body for the wire.
    pub fn encode(&self) -> Vec<u8> {
        match &self.body {
            ReplyBody::Response(response) => response.encode_to_vec(),
            ReplyBody::ServiceError(status) => status.encode_to_vec(),
        }
    }

    fn faulted(class: FaultClass, error: &dyn StdError) -> Self {
        Self::service_error(DecisionOutcome::Faulted, status::faulted(class, error))
    }

    fn from_case(decision: DecisionOutcome, case: DecideResponseCase) -> Self {
        Self {
            body: ReplyBody::Response(v1::DecideResponse { result: Some(case) }),
            decision,
        }
    }

    fn service_error(decision: DecisionOutcome, status: Status) -> Self {
        Self {
            body: ReplyBody::ServiceError(status),
            decision,
        }
    }
}

/// Carries one appended event back to the caller that submitted the command.
///
/// The stream stores the bare protobuf full name in its `Trogon-Event-Type`
/// header while `Any` requires the `type.googleapis.com/` prefix, so the prefix
/// is applied here and nowhere else.
fn decided_event(event: &Event) -> v1::DecidedEvent {
    v1::DecidedEvent {
        id: event.id.to_string(),
        event: MessageField::some(Any {
            type_url: format!("{TYPE_URL_PREFIX}{}", event.r#type),
            value: event.content.clone().into(),
            ..Any::default()
        }),
    }
}

fn carries_write_conflict(error: &(dyn StdError + 'static)) -> bool {
    let mut source = Some(error);
    while let Some(error) = source {
        if error.is::<OptimisticConcurrencyConflictError>() {
            return true;
        }
        source = error.source();
    }
    false
}

#[cfg(test)]
mod tests;
