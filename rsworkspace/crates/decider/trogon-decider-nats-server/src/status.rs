//! The `google.rpc.Status` bodies every outcome but an acceptance carries.
//!
//! [ADR#0016](../../../../../docs/adr/0016-protobuf-rpc-over-nats-micro-binding.md)
//! fixes the shape of an error body for the whole platform: one complete
//! `google.rpc.Status`, a canonical `google.rpc.Code`, and standard
//! `google.rpc` detail messages. This module is the decider's projection onto
//! that shape, and it is deliberately the only place in the host that decides
//! what code a failure reports as.
//!
//! What the decider adds on top is where the `Status` travels, not its shape.
//! 0016 keeps defined business outcomes off the micro error channel so
//! `num_errors` stays a health signal, so a rejection's `Status` rides in the
//! `Decide` response body while a fault's rides on the error channel. Both are
//! built here and both are a `Status`; only one of them is a service error.

use crate::constants::{CONCURRENCY_QUOTA_METRIC, DECIDER_ERROR_DOMAIN};
use buffa_types::google::protobuf::Any;
use std::error::Error as StdError;
use trogon_decider_runtime::{OverloadedError, UnauthorizedError};
use trogonai_proto::google::rpc::{Code, DebugInfo, ErrorInfo, QuotaFailure, Status, quota_failure};

/// The class of failure the host assigns when it could not execute a command.
///
/// Two things a bare `google.rpc.Code` cannot carry on its own: `Guest` and
/// `Internal` are both `INTERNAL` to a caller yet answerable by different
/// parties, and the retry advice differs across classes that share a code. The
/// class is what the host reasons in; the code and the reason are what it
/// reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultClass {
    /// No activated module claims the command type the subject names.
    Unroutable,
    /// The subject or the headers could not be read as a command at all.
    InvalidRequest,
    /// The caller asserted an expected revision no stream state can satisfy.
    ///
    /// Separate from [`Self::Conflict`] because the two look alike and differ
    /// on the only question a caller asks of a write failure: a contended
    /// append succeeds on a retry that replays the stream, and a fabricated
    /// revision never does.
    UnsatisfiablePrecondition,
    /// The stream moved under the command between the read and the append.
    Conflict,
    /// The guest faulted, trapped, or broke the WIT contract.
    Guest,
    /// The guest exceeded its wall-clock deadline.
    DeadlineExceeded,
    /// Event or snapshot storage failed.
    Storage,
    /// The host broke one of its own invariants.
    Internal,
}

impl FaultClass {
    /// The canonical code this class reports as.
    pub const fn code(self) -> Code {
        match self {
            Self::Unroutable => Code::UNIMPLEMENTED,
            Self::InvalidRequest | Self::UnsatisfiablePrecondition => Code::INVALID_ARGUMENT,
            Self::Conflict => Code::ABORTED,
            Self::Guest | Self::Internal => Code::INTERNAL,
            Self::DeadlineExceeded => Code::DEADLINE_EXCEEDED,
            Self::Storage => Code::UNAVAILABLE,
        }
    }

    /// The `ErrorInfo.reason` that tells this class apart from the others
    /// sharing its code.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Unroutable => "COMMAND_TYPE_UNROUTABLE",
            Self::InvalidRequest => "COMMAND_REQUEST_MALFORMED",
            Self::UnsatisfiablePrecondition => "EXPECTED_REVISION_UNSATISFIABLE",
            Self::Conflict => "STREAM_WRITE_CONFLICT",
            Self::Guest => "GUEST_FAULT",
            Self::DeadlineExceeded => "GUEST_DEADLINE_EXCEEDED",
            Self::Storage => "STORAGE_UNAVAILABLE",
            Self::Internal => "HOST_INTERNAL",
        }
    }
}

/// Builds the `faulted` arm's status from a class and the error that raised it.
pub fn faulted(class: FaultClass, error: &dyn StdError) -> Status {
    status(
        class.code(),
        error.to_string(),
        [
            Some(error_info(class.reason(), DECIDER_ERROR_DOMAIN)),
            debug_info(causal_chain(error.source())),
        ],
    )
}

/// Builds the `faulted` arm's status for a guest fault, whose chain crossed the
/// WIT boundary as pairs rather than as a live `#[source]` chain.
pub fn guest_faulted(message: String, chain: &[(String, String)]) -> Status {
    status(
        FaultClass::Guest.code(),
        message,
        [
            Some(error_info(FaultClass::Guest.reason(), DECIDER_ERROR_DOMAIN)),
            debug_info(flatten_pairs(chain)),
        ],
    )
}

/// Builds the `rejected` arm's status from what the module answered.
///
/// `FAILED_PRECONDITION` rather than `INVALID_ARGUMENT`: the command is
/// well-formed and would succeed against a different stream state, which is
/// precisely the distinction `google.rpc.Code` draws between the two. The
/// module's own code goes in `ErrorInfo.reason` under the module's name as the
/// domain, because that code space belongs to the module and not to the host.
pub fn rejected(module: &str, code: &str, message: String, chain: &[(String, String)]) -> Status {
    status(
        Code::FAILED_PRECONDITION,
        message,
        [Some(error_info(code, module)), debug_info(flatten_pairs(chain))],
    )
}

/// Builds the `shed` arm's status from the limit that was already committed.
///
/// The limit travels as a typed `QuotaFailure.Violation.quota_value` rather
/// than as text in `ErrorInfo.metadata`, so a caller sizing its backoff reads a
/// number. `subject` stays empty because the limit is the host's, not the
/// caller's: nothing about who asked determined that the answer was no.
pub fn shed(overloaded: OverloadedError) -> Status {
    let limit = overloaded.limit().as_usize();
    let violation = quota_failure::Violation {
        quota_metric: CONCURRENCY_QUOTA_METRIC.to_owned(),
        quota_value: i64::try_from(limit).unwrap_or(i64::MAX),
        description: format!("all {limit} concurrent executions are committed"),
        ..quota_failure::Violation::default()
    };

    status(
        Code::RESOURCE_EXHAUSTED,
        overloaded.to_string(),
        [
            Some(error_info("ADMISSION_LIMIT_REACHED", DECIDER_ERROR_DOMAIN)),
            Some(pack_detail(&QuotaFailure {
                violations: vec![violation],
            })),
        ],
    )
}

/// Builds the `denied` arm's status from the authorization refusal.
///
/// The two refusals are separate codes because the caller's next move differs:
/// `UNAUTHENTICATED` says present credentials, `PERMISSION_DENIED` says present
/// different ones. Collapsing them into one code, or into one prose reason,
/// would leave a caller to tell them apart by matching on a message.
///
/// The message is whatever the configured authorizer chose to say and the host
/// adds no denial vocabulary of its own, per
/// [ADR#0026](../../../../../docs/adr/0026-command-authorization-principal.md).
pub fn denied(unauthorized: &UnauthorizedError) -> Status {
    let (code, reason) = match unauthorized {
        UnauthorizedError::MissingPrincipal => (Code::UNAUTHENTICATED, "PRINCIPAL_MISSING"),
        UnauthorizedError::Denied(_) => (Code::PERMISSION_DENIED, "PRINCIPAL_UNAUTHORIZED"),
    };

    status(
        code,
        unauthorized.to_string(),
        [Some(error_info(reason, DECIDER_ERROR_DOMAIN)), None],
    )
}

/// Reads one standard detail message out of a status, if it carries one.
///
/// Offered here rather than left to each caller because `Status.details` is a
/// `repeated Any` with no ordering guarantee, and a caller that reaches for
/// `details[0]` is reading a position this module never promised.
pub fn find_detail<M: buffa::Message + buffa::MessageName>(status: &Status) -> Option<M> {
    status
        .details
        .iter()
        .find_map(|detail| detail.unpack_if::<M>(M::TYPE_URL).ok().flatten())
}

fn status(code: Code, message: String, details: [Option<Any>; 2]) -> Status {
    Status {
        code: code as i32,
        message,
        details: details.into_iter().flatten().collect(),
    }
}

fn error_info(reason: &str, domain: &str) -> Any {
    pack_detail(&ErrorInfo {
        reason: reason.to_owned(),
        domain: domain.to_owned(),
        ..ErrorInfo::default()
    })
}

/// A `DebugInfo` only when there is a chain to carry: an empty detail says the
/// same thing as an absent one and costs a caller a decode to find that out.
fn debug_info(stack_entries: Vec<String>) -> Option<Any> {
    if stack_entries.is_empty() {
        return None;
    }
    Some(pack_detail(&DebugInfo {
        stack_entries,
        ..DebugInfo::default()
    }))
}

fn pack_detail<M: buffa::Message + buffa::MessageName>(message: &M) -> Any {
    Any::pack(message, M::TYPE_URL)
}

/// Flattens a live `#[source]` chain, outermost cause first.
///
/// `DebugInfo.stack_entries` is an ordered list, which is the whole of what the
/// host used to encode by numbering keys `cause.0`, `cause.1`, and so on. The
/// index was never information the list did not already carry.
fn causal_chain(mut source: Option<&(dyn StdError + 'static)>) -> Vec<String> {
    let mut entries = Vec::new();
    while let Some(error) = source {
        entries.push(error.to_string());
        source = error.source();
    }
    entries
}

/// Flattens the pairs a guest's chain arrives as, keeping the guest's own
/// labels: they crossed the WIT boundary already flattened, and the host cannot
/// recover the chain they came from to re-derive them.
fn flatten_pairs(chain: &[(String, String)]) -> Vec<String> {
    chain.iter().map(|(key, value)| format!("{key}: {value}")).collect()
}

#[cfg(test)]
mod tests;
