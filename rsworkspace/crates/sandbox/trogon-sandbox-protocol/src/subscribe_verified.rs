use async_nats::{Client, SubscribeError, subject::ToSubject};

use crate::{ContextVerifiedSubscriber, DispatchVerifier, VerifiedSubscriber, global_verifier};

/// Subscribes like [`Client::subscribe`], refusing in `require` mode every signed-request message that fails
/// verification. A refused request with a reply subject gets an error body so its sender does not wait out a
/// timeout.
pub async fn subscribe_verified(
    client: &Client,
    subject: impl ToSubject,
) -> Result<VerifiedSubscriber, SubscribeError> {
    subscribe_with(client, subject, global_verifier()).await
}

/// [`subscribe_verified`] under a given verifier instead of the process-wide one.
pub async fn subscribe_with(
    client: &Client,
    subject: impl ToSubject,
    verifier: &'static DispatchVerifier,
) -> Result<VerifiedSubscriber, SubscribeError> {
    Ok(VerifiedSubscriber {
        inner: client.subscribe(subject).await?,
        client: client.clone(),
        verifier,
    })
}

/// [`subscribe_verified`] for a queue group.
pub async fn queue_subscribe_verified(
    client: &Client,
    subject: impl ToSubject,
    queue_group: impl Into<String>,
) -> Result<VerifiedSubscriber, SubscribeError> {
    Ok(VerifiedSubscriber {
        inner: client.queue_subscribe(subject, queue_group.into()).await?,
        client: client.clone(),
        verifier: global_verifier(),
    })
}

/// [`subscribe_verified`] yielding each message with its verified scope.
pub async fn subscribe_verified_context(
    client: &Client,
    subject: impl ToSubject,
) -> Result<ContextVerifiedSubscriber, SubscribeError> {
    subscribe_with_context(client, subject, global_verifier()).await
}

/// [`subscribe_verified_context`] under a given verifier instead of the process-wide one.
pub async fn subscribe_with_context(
    client: &Client,
    subject: impl ToSubject,
    verifier: &'static DispatchVerifier,
) -> Result<ContextVerifiedSubscriber, SubscribeError> {
    Ok(ContextVerifiedSubscriber {
        inner: subscribe_with(client, subject, verifier).await?,
    })
}
