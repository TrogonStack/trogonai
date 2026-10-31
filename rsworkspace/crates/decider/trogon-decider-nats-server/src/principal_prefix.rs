//! The prefix a client acting for one principal configures, per
//! [ADR#0082](../../../../../docs/adr/0082-bus-principal-from-subject-and-edge-proof-of-possession.md).
//!
//! A client names who it acts for by ending its subject prefix in that
//! principal's `{kind}.{id}` tokens. The subject a client derives from the
//! service and method names under that prefix is then one the host's
//! subscription matches, and one an allow list can grant to that principal
//! alone.

use trogon_decider_runtime::{PrincipalId, PrincipalKind};
use trogon_identity_types::{SubjectPrincipal, SubjectPrincipalError};
use trogon_nats::{SubjectViolationError, validate_published_subject};

use crate::constants::{DECIDE_METHOD_NAME, DECIDER_SERVICE_NAME};
use crate::endpoint::SubjectPrefix;

/// A subject prefix that ends in the principal a client acts for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalPrefix(String);

impl PrincipalPrefix {
    /// Appends a principal's subject tokens to the namespace a host answers under.
    ///
    /// The id is mapped the way every edge maps a verified credential's id, so
    /// one that cannot be a single subject token is carried in its hashed form,
    /// and that form is the id the host derives. The prefix answers to
    /// ADR#0055's token and byte budget for the `Decide` subject a client
    /// derives under it.
    pub fn new(prefix: &SubjectPrefix, kind: PrincipalKind, id: &PrincipalId) -> Result<Self, PrincipalPrefixError> {
        let principal = SubjectPrincipal::from_credential(kind, id).map_err(PrincipalPrefixError::Principal)?;
        let value = format!("{prefix}.{principal}");
        let subject = format!("{value}.{DECIDER_SERVICE_NAME}.{DECIDE_METHOD_NAME}");
        validate_published_subject(&subject).map_err(|source| PrincipalPrefixError::Subject { subject, source })?;

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

/// Why a principal does not yield a prefix a client can configure.
#[derive(Debug, thiserror::Error)]
pub enum PrincipalPrefixError {
    /// The id already has the hashed form the mapping reserves for its own
    /// output, so carrying it would claim whichever principal hashes to it.
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
