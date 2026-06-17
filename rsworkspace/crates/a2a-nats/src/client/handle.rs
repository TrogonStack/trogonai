//! `A2aClient` — JSON-RPC client facade over NATS request/reply + JetStream.
//!
//! The struct holds the request/reply path (config + agent id + NATS + JetStream)
//! plus the ingress overlay (talk to the agent directly, or route via a gateway
//! with a minted caller JWT). Per-operation methods (`message_send`, `tasks_get`,
//! `message_stream`, …) land in their dedicated PRs so each operation's wire
//! contract is reviewed on its own.

use a2a_identity_types::MintedUserJwt;

use crate::agent_id::A2aAgentId;

/// Whether `A2aClient` publishes to `{prefix}.agents.{agent_id}.…` (talking to the
/// agent directly on a trusted NATS connection) or to `{prefix}.gateway.…` with a
/// minted caller JWT (going through the policy edge).
#[derive(Clone, Debug)]
#[allow(dead_code)] // methods that read this land per-operation
enum ClientIngressTarget {
    AgentSubjects,
    GatewayIngress(MintedUserJwt),
}

#[derive(Clone)]
pub struct A2aClient<N, J> {
    #[allow(dead_code)] // methods that read these land per-operation
    agent_id: A2aAgentId,
    #[allow(dead_code)]
    nats: N,
    #[allow(dead_code)]
    js: J,
    #[allow(dead_code)]
    ingress: ClientIngressTarget,
}

impl<N, J> A2aClient<N, J> {
    pub fn new(agent_id: A2aAgentId, nats: N, js: J) -> Self {
        Self {
            agent_id,
            nats,
            js,
            ingress: ClientIngressTarget::AgentSubjects,
        }
    }

    /// Route requests through the policy gateway with a minted caller JWT.
    #[must_use]
    pub fn routing_via_gateway_ingress(mut self, caller_jwt: MintedUserJwt) -> Self {
        self.ingress = ClientIngressTarget::GatewayIngress(caller_jwt);
        self
    }

    /// Route requests directly to the agent's subjects (default).
    #[must_use]
    pub fn routing_to_agent(mut self) -> Self {
        self.ingress = ClientIngressTarget::AgentSubjects;
        self
    }

    pub fn agent_id(&self) -> &A2aAgentId {
        &self.agent_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_id() -> A2aAgentId {
        A2aAgentId::new("test-agent").unwrap()
    }

    fn minted_jwt() -> MintedUserJwt {
        MintedUserJwt::new("aaa.bbb.ccc").unwrap()
    }

    #[test]
    fn new_uses_agent_subjects_by_default() {
        let client = A2aClient::new(agent_id(), (), ());
        assert!(matches!(client.ingress, ClientIngressTarget::AgentSubjects));
    }

    #[test]
    fn routing_via_gateway_ingress_stores_jwt() {
        let client = A2aClient::new(agent_id(), (), ()).routing_via_gateway_ingress(minted_jwt());
        assert!(matches!(client.ingress, ClientIngressTarget::GatewayIngress(_)));
    }

    #[test]
    fn routing_to_agent_flips_back_to_agent_subjects() {
        let client = A2aClient::new(agent_id(), (), ())
            .routing_via_gateway_ingress(minted_jwt())
            .routing_to_agent();
        assert!(matches!(client.ingress, ClientIngressTarget::AgentSubjects));
    }

    #[test]
    fn agent_id_accessor_returns_constructor_value() {
        let client = A2aClient::new(agent_id(), (), ());
        assert_eq!(client.agent_id().as_str(), "test-agent");
    }
}
