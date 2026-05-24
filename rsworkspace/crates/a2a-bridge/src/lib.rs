pub mod auth;
pub mod error;
pub mod identity;
pub mod inbound;
pub mod outbound;

pub use auth::{
    AsyncNatsAuthMintWire, AuthCalloutClient, AuthCalloutJsonMintClient, StubAuthCalloutClient,
};
pub use error::BridgeError;
pub use identity::{BridgeUserJwt, CallerHttpsAuth};
pub use inbound::{
    AppState, AsyncNatsTokenGatewayUnary, AsyncNatsTokenTaskJetstream, GatewayInboundPublisher,
    InboundGatewayPublish, RecordingInboundPublisher, StubInboundGatewayPublish,
    StubTaskJetStreamPort, build_gateway_subject, default_a2a_prefix, gateway_router,
};
pub use outbound::{AgentRegistrationId, MethodSegment, OutboundHttpsAgentUpstream, StubOutboundForwarder, forward};
