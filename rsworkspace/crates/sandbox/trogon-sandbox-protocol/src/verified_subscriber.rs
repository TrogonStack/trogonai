//! A subscription that verifies signed broker requests before a worker sees them. Messages on subjects that are
//! not signed requests pass through, so a worker swaps `client.subscribe(subject)` for
//! `subscribe_verified(&client, subject)` and keeps reading the stream as before.

use std::pin::Pin;
use std::task::{Context, Poll};

use async_nats::{Client, Message, Subscriber};
use futures::Stream;

use crate::{DispatchVerifier, VerifiedVerbScope};

/// A subscription that refuses unverified signed requests.
pub struct VerifiedSubscriber {
    pub(crate) inner: Subscriber,
    pub(crate) client: Client,
    pub(crate) verifier: &'static DispatchVerifier,
}

impl VerifiedSubscriber {
    /// Stops the subscription, like [`Subscriber::unsubscribe`].
    pub async fn unsubscribe(&mut self) -> Result<(), async_nats::UnsubscribeError> {
        self.inner.unsubscribe().await
    }

    /// `Some` when the message may be handled, with its verified scope if it was verified. A refused request with a
    /// reply subject gets an error body so its sender does not wait out a timeout.
    pub(crate) fn admit(&self, message: &Message) -> Option<Option<VerifiedVerbScope>> {
        let error = match self.verifier.admit_message_scope(
            message.subject.as_str(),
            message.headers.as_ref(),
            &message.payload,
        ) {
            Ok(scope) => return Some(scope),
            Err(error) => error,
        };
        if let Some(reply) = message.reply.clone() {
            let client = self.client.clone();
            let body = serde_json::json!({
                "error": "dispatch_auth",
                "message": error.to_string(),
            });
            tokio::spawn(async move {
                let _ = client.publish(reply, body.to_string().into()).await;
            });
        }
        None
    }
}

impl Stream for VerifiedSubscriber {
    type Item = Message;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Message>> {
        loop {
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(message)) => {
                    if self.admit(&message).is_some() {
                        return Poll::Ready(Some(message));
                    }
                }
                other => return other,
            }
        }
    }
}
