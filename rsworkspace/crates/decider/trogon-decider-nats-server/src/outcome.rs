//! Projecting one execution onto the ADR#0057 reply.
//!
//! The reply body and the `Trogon-Decider-Outcome` header are produced from a
//! single value here rather than at two call sites. A discriminant derived
//! independently of the body it summarizes is a discriminant that can disagree
//! with it, and middleware that meters on the header would then be metering
//! something the caller never saw.

use buffa::{Message as _, MessageField};
use buffa_types::google::protobuf::Any;
use std::error::Error as StdError;
use trogon_decider_nats::OptimisticConcurrencyConflictError;
use trogon_decider_runtime::{Event, OverloadedError, UnauthorizedError};
use trogon_decider_wasm_runtime::{GuestDomainError, WasmCommandError, WasmExecutionResult};
use trogon_semconv::attribute::DecisionOutcome;
use trogonai_proto::decider::{CommandFaultedKind, CommandOutcomeCase, v1};

use crate::constants::TYPE_URL_PREFIX;

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
    pub fn rejected(error: &GuestDomainError) -> Self {
        Self {
            outcome: outcome(CommandOutcomeCase::Rejected(Box::new(v1::CommandRejected {
                code: error.code.clone(),
                message: error.message.clone(),
                details: guest_details(&error.details),
            }))),
            decision: DecisionOutcome::Rejected,
        }
    }

    /// The host was already at its admission limit.
    pub fn shed(overloaded: OverloadedError) -> Self {
        Self {
            outcome: outcome(CommandOutcomeCase::Shed(Box::new(v1::CommandShed {
                limit: u64::try_from(overloaded.limit().as_usize()).unwrap_or(u64::MAX),
            }))),
            decision: DecisionOutcome::Shed,
        }
    }

    /// The configured authorizer refused the submitting principal.
    pub fn denied(unauthorized: &UnauthorizedError) -> Self {
        Self {
            outcome: outcome(CommandOutcomeCase::Denied(Box::new(v1::CommandDenied {
                reason: unauthorized.to_string(),
            }))),
            decision: DecisionOutcome::Denied,
        }
    }

    /// No activated module claims the command type the subject named.
    pub fn unroutable(error: &dyn StdError) -> Self {
        Self::faulted(v1::command_faulted::Unroutable {}.into(), error)
    }

    /// The subject or the headers could not be read as a command at all.
    pub fn invalid_request(error: &dyn StdError) -> Self {
        Self::faulted(v1::command_faulted::InvalidRequest {}.into(), error)
    }

    /// The host broke one of its own invariants, so neither the guest nor
    /// storage is answerable for the failure.
    pub fn internal(error: &dyn StdError) -> Self {
        Self::faulted(v1::command_faulted::Internal {}.into(), error)
    }

    /// Maps the runtime's error taxonomy onto the reply, per ADR#0057 section 5.
    pub fn from_command_error<ReadSnapshotError, ReadStreamError, AppendStreamError>(
        error: &WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>,
    ) -> Self
    where
        ReadSnapshotError: StdError + 'static,
        ReadStreamError: StdError + 'static,
        AppendStreamError: StdError + 'static,
    {
        match error {
            WasmCommandError::Rejected(domain) => Self::rejected(domain),
            WasmCommandError::Overloaded(overloaded) => Self::shed(*overloaded),
            WasmCommandError::Unauthorized(unauthorized) => Self::denied(unauthorized),
            WasmCommandError::PreconditionConflict(_) => Self::faulted(v1::command_faulted::Conflict {}.into(), error),
            // A concurrent writer won the race. Reaching for it through the
            // append error's chain rather than the variant, because the store
            // reports it as one storage failure among many and only the cause
            // tells a retryable conflict apart from a broken JetStream.
            WasmCommandError::Append(append) if carries_write_conflict(append) => {
                Self::faulted(v1::command_faulted::Conflict {}.into(), error)
            }
            WasmCommandError::Faulted(domain)
            | WasmCommandError::Evolve(domain)
            | WasmCommandError::StreamId(domain) => Self::faulted_with_details(
                v1::command_faulted::Guest {}.into(),
                error.to_string(),
                guest_details(&domain.details),
            ),
            WasmCommandError::Trap(_) | WasmCommandError::EmptyDecision | WasmCommandError::Instantiate(_) => {
                Self::faulted(v1::command_faulted::Guest {}.into(), error)
            }
            WasmCommandError::DeadlineExceeded(_) => {
                Self::faulted(v1::command_faulted::DeadlineExceeded {}.into(), error)
            }
            WasmCommandError::ReadSnapshot(_) | WasmCommandError::ReadStream(_) | WasmCommandError::Append(_) => {
                Self::faulted(v1::command_faulted::Storage {}.into(), error)
            }
            WasmCommandError::ReplayLimitExceeded(_)
            | WasmCommandError::SnapshotAheadOfStream(_)
            | WasmCommandError::ReadAfterOverflow(_)
            | WasmCommandError::Blocking(_) => Self::faulted(v1::command_faulted::Internal {}.into(), error),
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

    fn faulted(kind: CommandFaultedKind, error: &dyn StdError) -> Self {
        Self::faulted_with_details(kind, error.to_string(), causal_chain(error.source()))
    }

    fn faulted_with_details(kind: CommandFaultedKind, message: String, details: Vec<v1::DomainErrorDetail>) -> Self {
        Self {
            outcome: outcome(CommandOutcomeCase::Faulted(Box::new(v1::CommandFaulted {
                message,
                details,
                kind: Some(kind),
            }))),
            decision: DecisionOutcome::Faulted,
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

fn guest_details(details: &[(String, String)]) -> Vec<v1::DomainErrorDetail> {
    details
        .iter()
        .map(|(key, value)| v1::DomainErrorDetail {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// Flattens a `#[source]` chain into ordered `cause.N` pairs, the same shape the
/// guest bridge produces, so a caller reads one detail vocabulary whether the
/// chain was built in the guest or in the host.
fn causal_chain(mut source: Option<&(dyn StdError + 'static)>) -> Vec<v1::DomainErrorDetail> {
    let mut details = Vec::new();
    let mut depth = 0usize;
    while let Some(error) = source {
        details.push(v1::DomainErrorDetail {
            key: format!("cause.{depth}"),
            value: error.to_string(),
        });
        source = error.source();
        depth += 1;
    }
    details
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
