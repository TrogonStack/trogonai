#![doc = include_str!("../README.md")]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod auth;
pub mod error;
pub mod identity;
pub mod inbound;
#[cfg(test)]
mod nats_transport_harness;
pub mod outbound;

pub use auth::{
    AsyncNatsAuthMintWire, AuthCalloutClient, AuthCalloutJsonMintClient, BridgeTenantAccount,
    InProcessCalloutDispatcherMintWire, StubAuthCalloutClient, StubAuthCalloutMint,
};
pub use error::BridgeError;
pub use identity::{BridgeAgentId, BridgeUserJwt, CallerHttpsAuth, MintedCallerId};
pub use inbound::{
    AppState, AsyncNatsTokenGatewayUnary, AsyncNatsTokenTaskJetstream, GatewayInboundPublisher, InboundGatewayPublish,
    RecordingInboundPublisher, StubInboundGatewayPublish, StubTaskJetStreamPort, build_gateway_subject,
    default_a2a_prefix, gateway_router,
};
pub use outbound::{AgentRegistrationId, MethodSegment, OutboundHttpsAgentUpstream, StubOutboundForwarder, forward};
