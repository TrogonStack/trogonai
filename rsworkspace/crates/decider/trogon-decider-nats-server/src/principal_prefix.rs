//! The prefix a client acting for one principal configures, per
//! [ADR#0082](../../../../../docs/adr/0082-bus-principal-from-subject-and-edge-proof-of-possession.md).
//!
//! A client names who it acts for by ending its subject prefix in that
//! principal's `{kind}.{id}` tokens. The subject a client derives from the
//! service and method names under that prefix is then one the host's
//! subscription matches, and one an allow list can grant to that principal
//! alone.

use trogon_decider_runtime::{PrincipalId, PrincipalKind};
use trogon_nats::constants::MAX_NATS_TOKEN_LENGTH;
use trogon_nats::{SubjectTokenViolationError, SubjectViolationError, validate_published_subject};

use crate::constants::{DECIDE_METHOD_NAME, DECIDER_SERVICE_NAME};
use crate::endpoint::SubjectPrefix;
use crate::subject_principal::{SubjectPrincipal, SubjectPrincipalError};

/// A subject prefix that ends in the principal a client acts for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalPrefix(String);

impl PrincipalPrefix {
    /// Appends a principal's subject tokens to the namespace a host answers under.
    ///
    /// The prefix answers to the limits of the `Decide` subject a client derives
    /// under it: each token within one token's length, and the whole subject
    /// within ADR#0055's token and byte budget.
    pub fn new(prefix: &SubjectPrefix, kind: PrincipalKind, id: &PrincipalId) -> Result<Self, PrincipalPrefixError> {
        let principal =
            SubjectPrincipal::from_tokens(kind.as_str(), id.as_str()).map_err(PrincipalPrefixError::Principal)?;
        let value = format!("{prefix}.{}.{}", principal.kind(), principal.principal_id());
        let subject = format!("{value}.{DECIDER_SERVICE_NAME}.{DECIDE_METHOD_NAME}");
        validate_published_subject(&subject)
            .and_then(|()| validate_token_lengths(&subject))
            .map_err(|source| PrincipalPrefixError::Subject { subject, source })?;

        Ok(Self(value))
    }

    /// Returns the prefix as a client configures it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PrincipalPrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

fn validate_token_lengths(subject: &str) -> Result<(), SubjectViolationError> {
    subject
        .split('.')
        .enumerate()
        .find(|(_, token)| token.len() > MAX_NATS_TOKEN_LENGTH)
        .map_or(Ok(()), |(index, token)| {
            Err(SubjectViolationError::Token {
                index,
                token: token.to_owned(),
                source: SubjectTokenViolationError::TooLong(token.len()),
            })
        })
}

/// Why a principal does not yield a prefix a client can configure.
#[derive(Debug, thiserror::Error)]
pub enum PrincipalPrefixError {
    /// The principal cannot be carried as subject tokens.
    #[error("principal cannot be carried in a subject: {0}")]
    Principal(#[source] SubjectPrincipalError),
    /// The subject a client derives under the prefix breaks ADR#0055's shape or limits.
    #[error("subject '{subject}' under the prefix is not conformant: {source}")]
    Subject {
        subject: String,
        #[source]
        source: SubjectViolationError,
    },
}

#[cfg(test)]
mod tests;
