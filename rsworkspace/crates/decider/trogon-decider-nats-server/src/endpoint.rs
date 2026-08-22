//! The subject the decider's one micro endpoint answers on.
//!
//! [ADR#0016](../../../../../docs/adr/0016-protobuf-rpc-over-nats-micro-binding.md)
//! derives an endpoint subject from the service and the method rather than from
//! the payload, so this is a single fixed subject and not a subtree. Which
//! command a delivery carries is named in its `DecideRequest`, which is what
//! lets a fixed descriptor serve a command set that changes under
//! `DeciderRegistryHandle::activate`.

use trogon_nats::{DottedNatsToken, SubjectTokenViolationError, SubjectViolationError, validate_published_subject};

use crate::constants::{DECIDE_METHOD_NAME, DECIDER_SERVICE_NAME};

/// The configured subject namespace a host answers under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectPrefix(DottedNatsToken);

impl SubjectPrefix {
    /// Creates a prefix after rejecting anything NATS cannot carry as a token.
    pub fn new(value: impl AsRef<str>) -> Result<Self, SubjectTokenViolationError> {
        DottedNatsToken::new(value).map(Self)
    }

    /// Returns the prefix as written.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for SubjectPrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.as_str().fmt(f)
    }
}

/// The `Decide` endpoint, bound to one configured prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEndpoint {
    prefix: SubjectPrefix,
    subject: String,
}

impl CommandEndpoint {
    /// Binds the endpoint to a prefix, rejecting a subject NATS could not carry.
    ///
    /// Checked here rather than at the first delivery: a host whose subject
    /// breaks ADR#0055's limits can never receive a command, and discovering
    /// that at startup is far cheaper than discovering it in production.
    pub fn new(prefix: SubjectPrefix) -> Result<Self, CommandEndpointError> {
        let subject = format!("{prefix}.{DECIDER_SERVICE_NAME}.{DECIDE_METHOD_NAME}");
        validate_published_subject(&subject).map_err(|source| CommandEndpointError::Subject {
            subject: subject.clone(),
            source,
        })?;

        Ok(Self { prefix, subject })
    }

    /// The prefix this endpoint is bound to.
    pub const fn prefix(&self) -> &SubjectPrefix {
        &self.prefix
    }

    /// The subject this endpoint subscribes on and callers publish to.
    pub fn subject(&self) -> &str {
        &self.subject
    }
}

/// Why a prefix does not yield a usable endpoint subject.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CommandEndpointError {
    /// The endpoint subject breaks ADR#0055's shape or limits.
    #[error("endpoint subject '{subject}' is not conformant: {source}")]
    Subject {
        subject: String,
        #[source]
        source: SubjectViolationError,
    },
}

#[cfg(test)]
mod tests;
