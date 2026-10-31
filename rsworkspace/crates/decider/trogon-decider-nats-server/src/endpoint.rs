//! The subject the decider's one micro endpoint answers on.
//!
//! [ADR#0016](../../../../../docs/adr/0016-protobuf-rpc-over-nats-micro-binding.md)
//! derives an endpoint subject from the service and the method rather than from
//! the payload. Which command a delivery carries is named in its
//! `DecideRequest`, which is what lets a fixed descriptor serve a command set
//! that changes under `DeciderRegistryHandle::activate`.
//!
//! [ADR#0082](../../../../../docs/adr/0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
//! puts who is acting into the subject as well: a caller's prefix ends in its
//! principal's `{kind}.{id}` tokens, so the endpoint is one subscription with
//! wildcards at those positions, and at the tenant position when the host
//! serves several tenants. The principal is read back from the subject the
//! server delivered, which the caller's allow list already constrained, and
//! never from anything the caller wrote into the message.

use trogon_decider_runtime::CommandPrincipal;
use trogon_nats::{DottedNatsToken, SubjectTokenViolationError, SubjectViolationError, validate_published_subject};

use crate::constants::{DECIDE_METHOD_NAME, DECIDER_SERVICE_NAME, NARROWEST_TOKEN, SUBJECT_TOKEN_WILDCARD};
use crate::subject_principal::{SubjectPrincipal, SubjectPrincipalError};
use crate::tenant_position::TenantPosition;

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

/// The `Decide` endpoint, bound to one configured prefix and tenant position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEndpoint {
    prefix: SubjectPrefix,
    tenant: TenantPosition,
    subscription: String,
}

impl CommandEndpoint {
    /// Binds the endpoint to a prefix and a tenant position, rejecting a
    /// subscription that no subject NATS could carry would match.
    ///
    /// Checked here rather than at the first delivery: a host whose subject
    /// breaks ADR#0055's limits can never receive a command, and discovering
    /// that at startup is far cheaper than discovering it in production. The
    /// check runs on the shortest subject the subscription matches, since a
    /// wildcard is not an address and every match spends the same tokens.
    pub fn new(prefix: SubjectPrefix, tenant: TenantPosition) -> Result<Self, CommandEndpointError> {
        let tokens = subscription_tokens(&prefix, tenant);
        let subscription = tokens.join(".");
        let narrowest = tokens
            .iter()
            .map(|token| match *token {
                SUBJECT_TOKEN_WILDCARD => NARROWEST_TOKEN,
                literal => literal,
            })
            .collect::<Vec<_>>()
            .join(".");
        validate_published_subject(&narrowest).map_err(|source| CommandEndpointError::Subject {
            subject: subscription.clone(),
            source,
        })?;

        Ok(Self {
            prefix,
            tenant,
            subscription,
        })
    }

    /// The prefix this endpoint is bound to.
    pub const fn prefix(&self) -> &SubjectPrefix {
        &self.prefix
    }

    /// Whether this endpoint's subjects lead with a tenant token.
    pub const fn tenant(&self) -> TenantPosition {
        self.tenant
    }

    /// The pattern this endpoint subscribes on.
    pub fn subscription(&self) -> &str {
        &self.subscription
    }

    /// Reads the principal a delivered subject names at this endpoint's
    /// principal positions.
    ///
    /// Anything that is not this endpoint's subject, or whose principal tokens
    /// are not a principal, is refused rather than read as nobody: an
    /// execution without a principal is exactly what ADR#0082 rules out.
    pub fn derive_principal(&self, subject: &str) -> Result<CommandPrincipal, CommandSubjectError> {
        let outside = || CommandSubjectError::OutsideEndpoint {
            subject: subject.to_owned(),
        };
        let mut tokens = subject.split('.');
        if self.tenant == TenantPosition::Wildcard && tokens.next().is_none_or(str::is_empty) {
            return Err(outside());
        }
        if !self
            .prefix
            .as_str()
            .split('.')
            .all(|expected| tokens.next() == Some(expected))
        {
            return Err(outside());
        }
        let (Some(kind), Some(id), Some(DECIDER_SERVICE_NAME), Some(DECIDE_METHOD_NAME), None) = (
            tokens.next(),
            tokens.next(),
            tokens.next(),
            tokens.next(),
            tokens.next(),
        ) else {
            return Err(outside());
        };
        let principal = SubjectPrincipal::from_tokens(kind, id).map_err(|source| CommandSubjectError::Principal {
            subject: subject.to_owned(),
            source,
        })?;

        Ok(CommandPrincipal::new(principal.kind(), principal.principal_id()))
    }
}

fn subscription_tokens(prefix: &SubjectPrefix, tenant: TenantPosition) -> Vec<&str> {
    let tenant = match tenant {
        TenantPosition::Absent => None,
        TenantPosition::Wildcard => Some(SUBJECT_TOKEN_WILDCARD),
    };
    tenant
        .into_iter()
        .chain([
            prefix.as_str(),
            SUBJECT_TOKEN_WILDCARD,
            SUBJECT_TOKEN_WILDCARD,
            DECIDER_SERVICE_NAME,
            DECIDE_METHOD_NAME,
        ])
        .collect()
}

/// Why a prefix does not yield a usable endpoint subscription.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CommandEndpointError {
    /// The shortest subject the subscription matches breaks ADR#0055's shape or limits.
    #[error("endpoint subject '{subject}' is not conformant: {source}")]
    Subject {
        subject: String,
        #[source]
        source: SubjectViolationError,
    },
}

/// Why a delivered subject names no principal this endpoint can act for.
#[derive(Debug, thiserror::Error)]
pub enum CommandSubjectError {
    /// The subject is not this endpoint's, so it has no principal positions.
    #[error("subject {subject:?} is not this endpoint's subject")]
    OutsideEndpoint { subject: String },
    /// The tokens at the principal positions do not name a principal.
    #[error("subject {subject:?} does not name a principal: {source}")]
    Principal {
        subject: String,
        #[source]
        source: SubjectPrincipalError,
    },
}

#[cfg(test)]
mod tests;
