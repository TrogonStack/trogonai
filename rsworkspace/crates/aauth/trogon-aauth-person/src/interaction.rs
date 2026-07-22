//! Pluggable interaction delivery, per "User Interaction" (#user-interaction)
//! and "Resource-Initiated Interaction" (#resource-initiated-interaction): how
//! the PS actually reaches the person (push notification, chat message,
//! email, etc) is deployment-specific.

use async_trait::async_trait;

use crate::error::InteractionRelayError;

/// One interaction to relay to the person, per "User Interaction". `url` is
/// where the person can act; `code` is the short human-verifiable code shown
/// alongside it, per "Interaction Code Format" (Crockford base32, >= 40 bits
/// entropy).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InteractionNotice {
    pub url: String,
    pub code: String,
    pub description: Option<String>,
}

/// Deployment-specific channel for reaching the person. Implementations
/// return [`InteractionRelayError::Unavailable`] when no channel currently
/// works (the caller falls back to directing the user itself) and
/// [`InteractionRelayError::UserUnreachable`] when this is terminal per
/// "Token Endpoint Error Codes" `user_unreachable`.
#[async_trait]
pub trait InteractionChannel: Send + Sync {
    async fn notify(&self, notice: &InteractionNotice) -> Result<(), InteractionRelayError>;
}

/// No-op channel: always reports [`InteractionRelayError::Unavailable`].
/// Useful as a default for deployments that only support agent-directed
/// interaction (the agent shows the URL/code itself) rather than PS-pushed
/// notifications.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopInteractionChannel;

#[async_trait]
impl InteractionChannel for NoopInteractionChannel {
    async fn notify(&self, _notice: &InteractionNotice) -> Result<(), InteractionRelayError> {
        Err(InteractionRelayError::Unavailable)
    }
}

#[cfg(test)]
mod tests;
