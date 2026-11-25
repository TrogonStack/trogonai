use trogon_identity_types::{SubjectPrincipal, SubjectPrincipalError};

use super::super::routed::{RoutedSplitError, RoutedTokens};
use crate::a2a_method::A2aMethod;
use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::{A2aAgentId, AgentIdError};

/// `{prefix}.v1.agents.{agent_id}.{kind}.{id}.{method…}`: a request delivered to an
/// agent for the caller whose principal sits at `{kind}.{id}`.
///
/// The agent derives its caller from this subject and from nothing else (ADR#0082).
/// A caller's allow list admits a publish only on subjects that embed its own
/// principal, and a relay writes the principal it parsed from the subject that
/// caused the publish, so [`Self::caller`] is authenticated by the subject the
/// request arrived on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDeliverySubject {
    prefix: A2aPrefix,
    agent_id: A2aAgentId,
    caller: SubjectPrincipal,
    method: A2aMethod,
}

impl AgentDeliverySubject {
    pub fn new(prefix: &A2aPrefix, agent_id: &A2aAgentId, caller: &SubjectPrincipal, method: A2aMethod) -> Self {
        Self {
            prefix: prefix.clone(),
            agent_id: agent_id.clone(),
            caller: caller.clone(),
            method,
        }
    }

    pub fn parse(prefix: &A2aPrefix, subject: &str) -> Result<Self, AgentDeliverySubjectError> {
        let tokens = RoutedTokens::split(prefix, "agents", subject).map_err(|error| match error {
            RoutedSplitError::ForeignFamily => AgentDeliverySubjectError::NotAgentDelivery,
            RoutedSplitError::MissingTokens => AgentDeliverySubjectError::BadSubjectShape,
        })?;
        let agent_id = A2aAgentId::new(tokens.route).map_err(AgentDeliverySubjectError::InvalidAgentId)?;
        let caller =
            SubjectPrincipal::from_tokens(tokens.kind, tokens.id).map_err(AgentDeliverySubjectError::InvalidCaller)?;
        let method = A2aMethod::from_dotted_suffix(tokens.terminal).ok_or(AgentDeliverySubjectError::UnknownMethod)?;
        Ok(Self {
            prefix: prefix.clone(),
            agent_id,
            caller,
            method,
        })
    }

    pub fn agent_id(&self) -> &A2aAgentId {
        &self.agent_id
    }

    pub fn caller(&self) -> &SubjectPrincipal {
        &self.caller
    }

    pub fn method(&self) -> &A2aMethod {
        &self.method
    }
}

impl std::fmt::Display for AgentDeliverySubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.v1.agents.{}.{}.{}",
            self.prefix.as_str(),
            self.agent_id.as_str(),
            self.caller,
            self.method.as_dotted_suffix()
        )
    }
}

impl async_nats::subject::ToSubject for AgentDeliverySubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Requestable for AgentDeliverySubject {}

impl super::super::stream::StreamAssignment for AgentDeliverySubject {
    const STREAM: Option<super::super::stream::A2aStream> = None;
}

/// Why a subject is not an agent delivery under the configured prefix.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AgentDeliverySubjectError {
    #[error("subject does not start with '{{prefix}}.v1.agents.' for the configured prefix")]
    NotAgentDelivery,
    #[error("expected '{{prefix}}.v1.agents.{{agent_id}}.{{kind}}.{{id}}.{{method…}}'")]
    BadSubjectShape,
    #[error("agent id token is invalid")]
    InvalidAgentId(#[source] AgentIdError),
    #[error("caller principal tokens are invalid")]
    InvalidCaller(#[source] SubjectPrincipalError),
    #[error("subject does not end in a known A2A method")]
    UnknownMethod,
}

#[cfg(test)]
mod tests;
