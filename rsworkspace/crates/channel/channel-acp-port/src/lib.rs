//! The ACP side of a chat channel bridge, shared by every channel binary.
//!
//! `trogon-channel` owns everything channel-neutral and knows no agent
//! protocol; each `channel-bridge-*` binary knows one platform. What sits
//! between them, reaching an ACP agent through `acp-nats`, is the same for
//! every platform, so it lives here once instead of being copied into each
//! bridge (see `docs/architecture/multi-channel-agent-routing.md`: "a second
//! channel imports the same brain; it never copies it").
//!
//! - [`AcpPort`] implements [`trogon_channel::AgentPort`] over the acp-nats
//!   `Bridge`.
//! - [`BufferedRenderClient`] is the ACP client half: it collects a session's
//!   streamed text until the turn ends and the bridge sends it.
//! - [`Pipeline`] carries one inbound message to the agent and its reply back;
//!   a bridge plugs in only its decoder and its outbound API.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod acp_port;
mod buffered_render_client;
mod pipeline;

pub use acp_port::{AcpBridge, AcpPort, AcpPortError, SessionMethod, SessionMethods};
pub use buffered_render_client::BufferedRenderClient;
pub use pipeline::{
    DecodeInbound, NEW_SESSION_ACKNOWLEDGEMENT, Pipeline, PipelineError, SendText, SendTyping, sender_endpoint,
};
