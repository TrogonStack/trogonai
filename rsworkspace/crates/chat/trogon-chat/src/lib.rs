//! Channel-neutral chat domain: the shared brain every channel bridge imports.
//!
//! See `docs/architecture/multi-channel-agent-routing.md`. This crate owns the
//! vocabulary (endpoints, principals, conversations, inbound events, render
//! commands), the JetStream KV registries, and the [`AgentPort`] trait through
//! which bridges reach agents. Channel binaries (e.g. `chat-bridge-telegram`)
//! contain platform I/O only; nothing in this crate may reference a specific
//! platform or agent protocol.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod agent_port;
pub mod conversation;
pub mod endpoint;
pub mod event;
pub mod render;
pub mod store;

pub use agent_port::{AgentPort, AgentSessionId, PromptOutcome};
pub use conversation::{AgentId, ConversationId, ConversationRecord};
pub use endpoint::{Endpoint, EndpointError, PrincipalId};
pub use event::{Attachment, InboundChatEvent, Sender};
pub use render::RenderCommand;
pub use store::{ChatStore, ChatStoreError};
