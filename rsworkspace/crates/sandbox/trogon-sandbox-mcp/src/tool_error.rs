use rmcp::model::{ContentBlock, IntoContents};
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{LeaseError, TerminalError};

use crate::constants::UNAUTHENTICATED_MSG;
use crate::policy::{CommandVetoError, PathVetoError};

/// Why a tool call failed. The message reaches the model as the tool result.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("{UNAUTHENTICATED_MSG}")]
    Unauthenticated,

    #[error("invalid {field}: {source}")]
    InvalidInput {
        field: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("invalid {field}: got {got}, expected {expected}")]
    InvalidChoice {
        field: &'static str,
        got: String,
        expected: &'static str,
    },

    #[error("{option} requires {requires}")]
    InvalidCombination {
        option: &'static str,
        requires: &'static str,
    },

    #[error("execution {0} is no longer known to the worker")]
    ExecutionUnknown(String),

    #[error("execution {0} did not finish in time")]
    ExecutionUnfinished(String),

    #[error("tool denied by server policy: {0}")]
    ToolDenied(String),

    #[error("command denied by server policy: {0}")]
    CommandDenied(#[source] CommandVetoError),

    #[error("path denied by server policy: {0}")]
    PathDenied(#[source] PathVetoError),

    #[error("NATS client not connected")]
    NotConnected,

    #[error("{0}")]
    Client(#[source] Box<ClientError>),

    #[error("failed to encode the reply: {0}")]
    Encode(#[source] serde_json::Error),

    #[error("the broker answered with an unexpected reply: {0}")]
    UnexpectedReply(&'static str),

    #[error("terminal: {0}")]
    Terminal(#[source] TerminalError),

    #[error("terminal: the lease's provider does not support terminals")]
    TerminalUnsupported,

    #[error("terminal: a terminal runs whatever is typed into it, which the command policy cannot check")]
    TerminalUnderCommandPolicy,

    #[error("terminal: no open terminal {0}")]
    TerminalNotOpen(String),

    #[error("terminal: could not allocate a terminal id: {0}")]
    TerminalId(#[source] trogon_sandbox_contract::TerminalIdError),
}

impl From<ClientError> for ToolError {
    fn from(error: ClientError) -> Self {
        Self::Client(Box::new(error))
    }
}

impl ToolError {
    /// Wraps the typed error of one input field.
    pub(crate) fn invalid(field: &'static str, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::InvalidInput {
            field,
            source: Box::new(source),
        }
    }

    /// A refusal the broker answered, shown with its retry classification.
    pub(crate) fn denied(error: LeaseError) -> Self {
        Self::Client(Box::new(ClientError::Denied(error)))
    }

    /// The message shown to the model. Broker errors append their classification so an agent can tell a retry from
    /// a dead end.
    fn text(&self) -> String {
        match self {
            Self::Client(e) => format!(
                "{e}\nretryable: {}; outcome_unknown: {}; hint: {}",
                e.retryable(),
                e.outcome_unknown(),
                e.hint()
            ),
            other => other.to_string(),
        }
    }
}

impl IntoContents for ToolError {
    fn into_contents(self) -> Vec<ContentBlock> {
        vec![ContentBlock::text(self.text())]
    }
}

#[cfg(test)]
mod tests;
