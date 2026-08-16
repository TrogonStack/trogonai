//! Projecting one execution onto the ADR#0057 reply.
//!
//! The reply body and the `Trogon-Decider-Outcome` header are produced from a
//! single value here rather than at two call sites. A discriminant derived
//! independently of the body it summarizes is a discriminant that can disagree
//! with it, and middleware that meters on the header would then be metering
//! something the caller never saw.
//!
//! This module decides which arm an error belongs in. What that arm's
//! `google.rpc.Status` looks like belongs to [`crate::status`].

use buffa::{Message as _, MessageField};
use buffa_types::google::protobuf::Any;
use std::error::Error as StdError;
use trogon_decider_nats::OptimisticConcurrencyConflictError;
use trogon_decider_runtime::Event;
use trogon_decider_wasm_runtime::{GuestDomainError, ModuleName, WasmCommandError, WasmExecutionResult};
use trogon_semconv::attribute::DecisionOutcome;
use trogonai_proto::decider::{CommandOutcomeCase, v1};
use trogonai_proto::google::rpc::Status;

use crate::constants::TYPE_URL_PREFIX;
use crate::status::{self, FaultClass};

/// One command's outcome, in both forms the reply carries it.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandReply {
    outcome: v1::CommandOutcome,
    decision: DecisionOutcome,
}

impl CommandReply {
    /// The command was decided and its events appended.
    pub fn decided(result: &WasmExecutionResult) -> Self {
        Self {
            outcome: outcome(CommandOutcomeCase::Decided(Box::new(v1::CommandAccepted {
                stream_position: result.stream_position.as_u64(),
                events: result.events.iter().map(decided_event).collect(),
            }))),
            decision: DecisionOutcome::Decided,
        }
    }

    /// The module answered no.
    pub fn rejected(module: &ModuleName, error: &GuestDomainError) -> Self {
        Self::from_status(
            CommandOutcomeCase::Rejected,
            DecisionOutcome::Rejected,
            status::rejected(module.as_str(), &error.code, error.message.clone(), &error.details),
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
            WasmCommandError::Overloaded(overloaded) => Self::from_status(
                CommandOutcomeCase::Shed,
                DecisionOutcome::Shed,
                status::shed(*overloaded),
            ),
            WasmCommandError::Unauthorized(unauthorized) => Self::from_status(
                CommandOutcomeCase::Denied,
                DecisionOutcome::Denied,
                status::denied(unauthorized),
            ),
            WasmCommandError::PreconditionConflict(_) => Self::faulted(FaultClass::Conflict, error),
            // A concurrent writer won the race. Reaching for it through the
            // append error's chain rather than the variant, because the store
            // reports it as one storage failure among many and only the cause
            // tells a retryable conflict apart from a broken JetStream.
            WasmCommandError::Append(append) if carries_write_conflict(append) => {
                Self::faulted(FaultClass::Conflict, error)
            }
            WasmCommandError::Faulted(domain)
            | WasmCommandError::Evolve(domain)
            | WasmCommandError::StreamId(domain) => Self::from_status(
                CommandOutcomeCase::Faulted,
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
    pub const fn decision(&self) -> DecisionOutcome {
        self.decision
    }

    /// The `Trogon-Decider-Outcome` header value for this reply.
    pub const fn header_value(&self) -> &'static str {
        self.decision.as_str()
    }

    /// The reply body.
    pub const fn outcome(&self) -> &v1::CommandOutcome {
        &self.outcome
    }

    /// Encodes the reply body for the wire.
    pub fn encode(&self) -> Vec<u8> {
        self.outcome.encode_to_vec()
    }

    fn faulted(class: FaultClass, error: &dyn StdError) -> Self {
        Self::from_status(
            CommandOutcomeCase::Faulted,
            DecisionOutcome::Faulted,
            status::faulted(class, error),
        )
    }

    fn from_status(arm: fn(Box<Status>) -> CommandOutcomeCase, decision: DecisionOutcome, status: Status) -> Self {
        Self {
            outcome: outcome(arm(Box::new(status))),
            decision,
        }
    }
}

const fn outcome(case: CommandOutcomeCase) -> v1::CommandOutcome {
    v1::CommandOutcome { outcome: Some(case) }
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
