use trogon_identity_types::{SubjectPrincipal, SubjectPrincipalError};

use super::super::agents::AgentDeliverySubject;
use super::super::routed::{RoutedSplitError, RoutedTokens};
use crate::a2a_method::A2aMethod;
use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::{A2aAgentId, AgentIdError};

/// `{prefix}.v1.gateway.{agent_id}.{kind}.{id}.{method…}`: a request a caller sends
/// through the gateway to an agent.
///
/// A caller's allow list admits a publish here only with its own principal at
/// `{kind}.{id}`, so the gateway takes the caller from [`Self::caller`] and relays
/// the request on [`Self::agent_delivery`], which carries that same principal
/// (ADR#0082).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayIngressSubject {
    prefix: A2aPrefix,
    agent_id: A2aAgentId,
    caller: SubjectPrincipal,
    method: A2aMethod,
}

impl GatewayIngressSubject {
    pub fn new(prefix: &A2aPrefix, agent_id: &A2aAgentId, caller: &SubjectPrincipal, method: A2aMethod) -> Self {
        Self {
            prefix: prefix.clone(),
            agent_id: agent_id.clone(),
            caller: caller.clone(),
            method,
        }
    }

    pub fn parse(prefix: &A2aPrefix, subject: &str) -> Result<Self, GatewayIngressError> {
        let tokens = RoutedTokens::split(prefix, "gateway", subject).map_err(|error| match error {
            RoutedSplitError::ForeignFamily => GatewayIngressError::NotGatewayIngress,
            RoutedSplitError::MissingTokens => GatewayIngressError::BadSubjectShape,
        })?;
        let agent_id = A2aAgentId::new(tokens.route).map_err(GatewayIngressError::InvalidAgentId)?;
        let caller =
            SubjectPrincipal::from_tokens(tokens.kind, tokens.id).map_err(GatewayIngressError::InvalidCaller)?;
        let method = A2aMethod::from_dotted_suffix(tokens.terminal).ok_or(GatewayIngressError::UnknownMethodSuffix)?;
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

    /// The agent delivery this request is relayed on, naming the caller parsed
    /// from this subject.
    pub fn agent_delivery(&self) -> AgentDeliverySubject {
        AgentDeliverySubject::new(&self.prefix, &self.agent_id, &self.caller, self.method.clone())
    }
}

impl std::fmt::Display for GatewayIngressSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.v1.gateway.{}.{}.{}",
            self.prefix.as_str(),
            self.agent_id.as_str(),
            self.caller,
            self.method.as_dotted_suffix()
        )
    }
}

impl async_nats::subject::ToSubject for GatewayIngressSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Requestable for GatewayIngressSubject {}

impl super::super::stream::StreamAssignment for GatewayIngressSubject {
    const STREAM: Option<super::super::stream::A2aStream> = None;
}

/// Why a subject is not gateway ingress under the configured prefix.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GatewayIngressError {
    #[error("subject does not start with '{{prefix}}.v1.gateway.' for the configured prefix")]
    NotGatewayIngress,
    #[error("expected '{{prefix}}.v1.gateway.{{agent_id}}.{{kind}}.{{id}}.{{method…}}'")]
    BadSubjectShape,
    #[error("agent id segment fails NATS token validation")]
    InvalidAgentId(#[source] AgentIdError),
    #[error("caller principal tokens are invalid")]
    InvalidCaller(#[source] SubjectPrincipalError),
    #[error("unknown method suffix after the caller principal")]
    UnknownMethodSuffix,
}

#[cfg(test)]
mod tests;
