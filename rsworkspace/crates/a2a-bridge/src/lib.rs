#![doc = include_str!("../README.md")]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod auth;
pub mod error;
pub mod identity;
pub mod outbound;

pub use auth::{
    AsyncNatsAuthMintWire, AuthCalloutClient, AuthCalloutJsonMintClient, BridgeTenantAccount,
    InProcessCalloutDispatcherMintWire, StubAuthCalloutClient, StubAuthCalloutMint,
};
pub use error::BridgeError;
pub use identity::{BridgeAgentId, BridgeUserJwt, CallerHttpsAuth, MintedCallerId};
pub use outbound::{AgentRegistrationId, MethodSegment, OutboundHttpsAgentUpstream, StubOutboundForwarder, forward};
