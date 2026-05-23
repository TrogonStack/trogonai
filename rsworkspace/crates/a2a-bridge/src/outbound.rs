//! Symmetric egress: **`a2a.agent.{agent_id}.>` → HTTPS** for catalog-registered proxies.

use std::fmt;

use async_trait::async_trait;
use bytes::Bytes;

use crate::error::BridgeError;

/// HTTP upstream for an externally hosted A2A agent reached from the NATS-facing bridge.
#[async_trait]
pub trait OutboundHttpsAgentUpstream: Send + Sync {
    async fn proxy_jsonrpc_post(
        &self,
        agent_id: AgentRegistrationId,
        method: MethodSegment,
        body: Bytes,
    ) -> Result<Bytes, BridgeError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StubOutboundForwarder;

#[async_trait]
impl OutboundHttpsAgentUpstream for StubOutboundForwarder {
    async fn proxy_jsonrpc_post(
        &self,
        agent_id: AgentRegistrationId,
        method: MethodSegment,
        _body: Bytes,
    ) -> Result<Bytes, BridgeError> {
        let _: (AgentRegistrationId, MethodSegment) = (agent_id, method);
        unimplemented!("wired when outbound HTTPS path is provisioned against catalog-registered proxies")
    }
}

/// Agent identifier as registered via the registrar (catalog key).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentRegistrationId(String);

impl AgentRegistrationId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentRegistrationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// JSON-RPC method path segment with `/` as in A2A wire names (`message/send`, …).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MethodSegment(String);

impl MethodSegment {
    pub fn new(segment: impl Into<String>) -> Self {
        Self(segment.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MethodSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

pub async fn forward<U: OutboundHttpsAgentUpstream + ?Sized>(
    upstream: &U,
    agent_id: AgentRegistrationId,
    method: MethodSegment,
    body: Bytes,
) -> Result<Bytes, BridgeError> {
    upstream.proxy_jsonrpc_post(agent_id, method, body).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{BridgeUserJwt, CallerHttpsAuth};

    #[test]
    fn identity_newtypes_roundtrip() {
        let j = BridgeUserJwt::new("acct-user-jwt".to_owned());
        assert_eq!(j.as_str(), "acct-user-jwt");
        assert_eq!(j.into_inner(), "acct-user-jwt");

        let c = CallerHttpsAuth::new("Bearer x".to_owned());
        assert_eq!(c.into_inner(), "Bearer x");
    }

    #[test]
    fn agent_and_method_segments_preserve_opaque_strings() {
        let aid = AgentRegistrationId::new("ext-support".to_owned());
        let om = MethodSegment::new("message/send".to_owned());
        assert_eq!(aid.to_string(), "ext-support");
        assert_eq!(om.to_string(), "message/send");
    }
}
