use std::fmt::Debug;

use trogon_sandbox_contract::{RunCommandRequestError, SandboxSpecError};
use trogon_sandbox_protocol::{DispatchSignError, DispatchSignerConfigError, NatsTransferError};

use crate::AssignmentFaultError;

/// Why a check did not pass.
#[derive(Debug, thiserror::Error)]
pub enum CheckFailureError {
    /// The request did not reach the worker or the reply did not come back in time.
    #[error("request to {subject} failed: {source}")]
    Transport {
        subject: String,
        #[source]
        source: NatsTransferError,
    },
    /// A request body could not be built or encoded.
    #[error("could not encode a request: {0}")]
    Encode(#[source] serde_json::Error),
    /// The worker answered with a body the contract does not describe.
    #[error("unexpected reply on {subject}: {source}: {reply}")]
    Decode {
        subject: String,
        #[source]
        source: serde_json::Error,
        reply: String,
    },
    /// The request could not be signed.
    #[error("could not sign a request: {0}")]
    Sign(#[from] DispatchSignError),
    /// The signing key in the environment is unusable.
    #[error("the signing key is unusable: {0}")]
    SignerConfig(#[from] DispatchSignerConfigError),
    /// A value the suite generated is not a valid identifier.
    #[error("invalid identifier: {0}")]
    Identifier(String),
    /// The spec the suite built is not valid.
    #[error("invalid spec: {0}")]
    Spec(#[from] SandboxSpecError),
    /// The command request the suite built is not valid.
    #[error("invalid command request: {0}")]
    Command(#[from] RunCommandRequestError),
    /// The request or grant does not match the retained worker assignment.
    #[error("{0}")]
    Assignment(#[from] AssignmentFaultError),
    /// The worker answered, but not as the check requires.
    #[error("expected {expected}, observed {observed}")]
    Mismatch { expected: String, observed: String },
}

impl CheckFailureError {
    pub(crate) fn unexpected(expected: impl Into<String>, observed: &impl Debug) -> Self {
        Self::Mismatch {
            expected: expected.into(),
            observed: format!("{observed:?}"),
        }
    }

    pub(crate) fn identifier(error: impl std::fmt::Display) -> Self {
        Self::Identifier(error.to_string())
    }
}

/// `Ok` when `condition` holds, otherwise a mismatch naming what was observed.
pub(crate) fn ensure(
    condition: bool,
    expected: impl Into<String>,
    observed: &impl Debug,
) -> Result<(), CheckFailureError> {
    condition
        .then_some(())
        .ok_or_else(|| CheckFailureError::unexpected(expected, observed))
}

#[cfg(test)]
mod tests;
