//! `A2aClient` — JSON-RPC client facade over NATS request/reply + JetStream.
//!
//! The struct holds the request/reply path (prefix + agent id + NATS + JetStream)
//! plus the ingress overlay (talk to the agent directly, or route via a gateway
//! with a minted caller JWT). Per-operation methods (`message_send`, `tasks_get`,
//! `message_stream`, …) land in their dedicated PRs so each operation's wire
//! contract is reviewed on its own.

use std::time::Duration;

use a2a_identity_types::MintedUserJwt;

use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;
use crate::constants::{DEFAULT_OPERATION_TIMEOUT, MIN_TIMEOUT_SECS};
use crate::gateway_ingress::gateway_ingress_subject_from_agent_subject;

use super::error::ClientError;

/// Whether `A2aClient` publishes to `{prefix}.agents.{agent_id}.…` (talking to the
/// agent directly on a trusted NATS connection) or to `{prefix}.gateway.…` with a
/// minted caller JWT (going through the policy edge).
#[derive(Clone, Debug)]
#[allow(dead_code)] // GatewayIngress JWT is unwrapped by per-operation methods that land afterward
enum ClientIngressTarget {
    AgentSubjects,
    GatewayIngress(MintedUserJwt),
}

#[derive(Clone)]
#[allow(dead_code)] // prefix/nats/js/ingress are read by per-operation methods that land afterward
pub struct A2aClient<N, J> {
    prefix: A2aPrefix,
    agent_id: A2aAgentId,
    operation_timeout: Duration,
    nats: N,
    js: J,
    ingress: ClientIngressTarget,
}

impl<N, J> A2aClient<N, J> {
    pub fn new(prefix: A2aPrefix, agent_id: A2aAgentId, nats: N, js: J) -> Self {
        Self {
            prefix,
            agent_id,
            operation_timeout: DEFAULT_OPERATION_TIMEOUT,
            nats,
            js,
            ingress: ClientIngressTarget::AgentSubjects,
        }
    }

    #[must_use]
    pub fn with_operation_timeout(mut self, timeout: Duration) -> Self {
        self.operation_timeout = timeout.max(Duration::from_secs(MIN_TIMEOUT_SECS));
        self
    }

    /// Routes unary / bootstrap RPCs through `a2a-gateway`: subjects become
    /// `{prefix}.gateway.{agent_id}.{method…}` (see
    /// [`gateway_ingress_subject_from_agent_subject`]), with `caller_jwt`
    /// attached as the caller-JWT header on every gateway publish.
    ///
    /// Refresh and replace the JWT on the client when a per-operation call
    /// returns [`ClientError::GatewayCallerJwtExpired`].
    #[must_use]
    pub fn routing_via_gateway_ingress(mut self, caller_jwt: MintedUserJwt) -> Self {
        self.ingress = ClientIngressTarget::GatewayIngress(caller_jwt);
        self
    }

    /// Default (direct) routing to `{prefix}.agents.{agent_id}.{method…}`.
    #[must_use]
    pub fn routing_to_agent(mut self) -> Self {
        self.ingress = ClientIngressTarget::AgentSubjects;
        self
    }

    pub fn agent_id(&self) -> &A2aAgentId {
        &self.agent_id
    }

    #[allow(dead_code)] // called by per-operation methods that land afterward
    pub(super) fn prefix(&self) -> &A2aPrefix {
        &self.prefix
    }

    #[allow(dead_code)] // called by per-operation methods that land afterward
    pub(super) fn operation_timeout(&self) -> Duration {
        self.operation_timeout
    }

    #[allow(dead_code)] // called by per-operation methods that land afterward
    pub(super) fn gateway_caller_jwt(&self) -> Option<&MintedUserJwt> {
        match &self.ingress {
            ClientIngressTarget::AgentSubjects => None,
            ClientIngressTarget::GatewayIngress(jwt) => Some(jwt),
        }
    }

    #[allow(dead_code)] // called by per-operation methods that land afterward
    pub(super) fn outbound_rpc_subject(&self, agent_subject: String) -> Result<String, ClientError> {
        match &self.ingress {
            ClientIngressTarget::AgentSubjects => Ok(agent_subject),
            ClientIngressTarget::GatewayIngress(_) => {
                gateway_ingress_subject_from_agent_subject(&agent_subject, &self.prefix)
                    .ok_or(ClientError::InvalidRpcSubjectOverlay)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix() -> A2aPrefix {
        A2aPrefix::new("a2a").unwrap()
    }

    fn agent_id() -> A2aAgentId {
        A2aAgentId::new("test-agent").unwrap()
    }

    fn minted_jwt() -> MintedUserJwt {
        MintedUserJwt::new("aaa.bbb.ccc").unwrap()
    }

    #[test]
    fn new_uses_agent_subjects_by_default() {
        let client = A2aClient::new(prefix(), agent_id(), (), ());
        assert!(matches!(client.ingress, ClientIngressTarget::AgentSubjects));
        assert!(client.gateway_caller_jwt().is_none());
    }

    #[test]
    fn new_uses_default_operation_timeout() {
        let client = A2aClient::new(prefix(), agent_id(), (), ());
        assert_eq!(client.operation_timeout(), DEFAULT_OPERATION_TIMEOUT);
    }

    #[test]
    fn with_operation_timeout_overrides_default() {
        let client = A2aClient::new(prefix(), agent_id(), (), ()).with_operation_timeout(Duration::from_secs(7));
        assert_eq!(client.operation_timeout(), Duration::from_secs(7));
    }

    #[test]
    fn with_operation_timeout_clamps_below_minimum_to_minimum() {
        let client = A2aClient::new(prefix(), agent_id(), (), ()).with_operation_timeout(Duration::ZERO);
        assert_eq!(client.operation_timeout(), Duration::from_secs(MIN_TIMEOUT_SECS));
    }

    #[test]
    fn routing_via_gateway_ingress_stores_jwt() {
        let client = A2aClient::new(prefix(), agent_id(), (), ()).routing_via_gateway_ingress(minted_jwt());
        assert!(matches!(client.ingress, ClientIngressTarget::GatewayIngress(_)));
        assert!(client.gateway_caller_jwt().is_some());
    }

    #[test]
    fn routing_to_agent_flips_back_to_agent_subjects() {
        let client = A2aClient::new(prefix(), agent_id(), (), ())
            .routing_via_gateway_ingress(minted_jwt())
            .routing_to_agent();
        assert!(matches!(client.ingress, ClientIngressTarget::AgentSubjects));
    }

    #[test]
    fn agent_id_accessor_returns_constructor_value() {
        let client = A2aClient::new(prefix(), agent_id(), (), ());
        assert_eq!(client.agent_id().as_str(), "test-agent");
    }

    #[test]
    fn outbound_rpc_subject_returns_agent_subject_for_default_routing() {
        let client = A2aClient::new(prefix(), agent_id(), (), ());
        let subject = client
            .outbound_rpc_subject("a2a.agents.test-agent.message.send".to_string())
            .unwrap();
        assert_eq!(subject, "a2a.agents.test-agent.message.send");
    }

    #[test]
    fn outbound_rpc_subject_swaps_agents_to_gateway_when_jwt_set() {
        let client = A2aClient::new(prefix(), agent_id(), (), ()).routing_via_gateway_ingress(minted_jwt());
        let subject = client
            .outbound_rpc_subject("a2a.agents.test-agent.message.send".to_string())
            .unwrap();
        assert_eq!(subject, "a2a.gateway.test-agent.message.send");
    }

    #[test]
    fn outbound_rpc_subject_returns_invalid_overlay_for_non_agent_subject() {
        let client = A2aClient::new(prefix(), agent_id(), (), ()).routing_via_gateway_ingress(minted_jwt());
        let err = client
            .outbound_rpc_subject("wrong.prefix.test-agent.message.send".to_string())
            .unwrap_err();
        assert!(matches!(err, ClientError::InvalidRpcSubjectOverlay));
    }

    #[test]
    fn prefix_accessor_returns_constructor_value() {
        let client = A2aClient::new(prefix(), agent_id(), (), ());
        assert_eq!(client.prefix().as_str(), "a2a");
    }
}
