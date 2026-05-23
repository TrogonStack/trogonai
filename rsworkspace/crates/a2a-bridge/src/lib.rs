pub mod auth;
pub mod error;
pub mod identity;
pub mod inbound;
pub mod outbound;

pub use auth::{AuthCalloutClient, StubAuthCalloutClient};
pub use error::BridgeError;
pub use identity::{BridgeUserJwt, CallerHttpsAuth};
pub use inbound::{
    AppState, InboundGatewayPublish, RecordingInboundPublisher, StubInboundGatewayPublish, build_gateway_subject,
    gateway_router,
};
pub use outbound::{AgentRegistrationId, MethodSegment, OutboundHttpsAgentUpstream, StubOutboundForwarder, forward};
