use async_nats::Message;

use crate::VerifiedVerbScope;

/// A message a [`ContextVerifiedSubscriber`](crate::ContextVerifiedSubscriber) admitted, with what its signature
/// bound it to when it was verified.
pub struct VerifiedRequest {
    /// The received message.
    pub message: Message,
    /// The verified scope; `None` for a message that is not a signed request or passed in warn mode.
    pub scope: Option<VerifiedVerbScope>,
}
