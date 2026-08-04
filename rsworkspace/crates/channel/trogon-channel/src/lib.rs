//! Channel-neutral agent routing: the shared brain every channel bridge imports.
//!
//! See `docs/architecture/multi-channel-agent-routing.md`. This crate owns the
//! vocabulary (endpoints, principals, conversations, inbound events, render
//! commands), the JetStream KV registries, and the [`AgentPort`] trait through
//! which bridges reach agents. Channel binaries (e.g. `channel-bridge-telegram`)
//! contain platform I/O only; nothing in this crate may reference a specific
//! platform or agent protocol.
//!
//! A surface belongs here when it carries discrete messages to a peer that
//! stays addressable between them, and the sender's identity is foreign to the
//! agent. Chat apps qualify, and so do email, SMS, and push. A surface that
//! owns a workspace and can prompt its own user (a desktop app, an editor)
//! does not: it is an agent protocol client already and needs none of this.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod agent_port;
pub mod command;
pub mod command_trigger;
pub mod command_trigger_input;
pub mod conversation;
pub mod endpoint;
pub mod event;
pub mod render;
pub mod safe_token;
pub mod store;

pub use agent_port::{
    AgentPort, AgentPortError, AgentSessionId, PromptOutcome, ReleaseReason, ReleaseStep, SessionRelease,
};
pub use command::{Command, CommandTriggers, ParsedText};
pub use command_trigger::{CommandTrigger, CommandTriggerError};
pub use command_trigger_input::CommandTriggerInput;
pub use conversation::{AgentId, ConversationId, ConversationRecord};
pub use endpoint::{Endpoint, EndpointError, PrincipalId};
pub use event::{
    Attachment, AttachmentKind, EventFieldError, InboundEvent, MessageRef, MimeType, PlatformRef, PlatformUserId,
    Sender,
};
pub use render::RenderCommand;
pub use safe_token::{SafeToken, SafeTokenError};
pub use store::{ChannelStore, ChannelStoreError};
