//! The prefix a client acting for one principal configures, per
//! [ADR#0082](../../../../../docs/adr/0082-bus-principal-from-subject-and-edge-proof-of-possession.md).
//!
//! A client names who it acts for by ending its subject prefix in that
//! principal's `{kind}.{id}` tokens. The subject a client derives from the
//! service and method names under that prefix is then one the host's
//! subscription matches, and one an allow list can grant to that principal
//! alone.

use trogon_decider_runtime::{PrincipalId, PrincipalKind};
use trogon_nats::{DottedNatsToken, SubjectTokenViolationError};

use crate::endpoint::SubjectPrefix;
use crate::subject_principal::{SubjectPrincipal, SubjectPrincipalError};

/// A subject prefix that ends in the principal a client acts for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalPrefix(DottedNatsToken);

impl PrincipalPrefix {
    /// Appends a principal's subject tokens to the namespace a host answers under.
    pub fn new(prefix: &SubjectPrefix, kind: PrincipalKind, id: &PrincipalId) -> Result<Self, PrincipalPrefixError> {
        let principal =
            SubjectPrincipal::from_tokens(kind.as_str(), id.as_str()).map_err(PrincipalPrefixError::Principal)?;
        DottedNatsToken::new(format!("{prefix}.{}.{}", principal.kind(), principal.principal_id()))
            .map(Self)
            .map_err(PrincipalPrefixError::Prefix)
    }

    /// Returns the prefix as a client configures it.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for PrincipalPrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.as_str().fmt(f)
    }
}

/// Why a principal does not yield a prefix a client can configure.
#[derive(Debug, thiserror::Error)]
pub enum PrincipalPrefixError {
    /// The principal cannot be carried as subject tokens.
    #[error("principal cannot be carried in a subject: {0}")]
    Principal(#[source] SubjectPrincipalError),
    /// The prefix with the principal appended is not a prefix NATS can carry.
    #[error("prefix ending in the principal is not a subject prefix: {0}")]
    Prefix(#[source] SubjectTokenViolationError),
}

#[cfg(test)]
mod tests;
