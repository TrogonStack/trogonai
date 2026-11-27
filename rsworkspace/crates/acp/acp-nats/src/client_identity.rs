//! Which client a request on the agent subjects comes from.
//!
//! NATS does not tell a responder who sent a request: the reply inbox is
//! random and the connection is not visible to the subscriber. The only
//! identity an ACP client gives is the `clientInfo.name` in its `initialize`,
//! and that reaches the agent once. So the [`Bridge`](crate::Bridge) carries it
//! on: once its client initializes, every request and notification it sends
//! (`initialize` included) has the [`ACP_CLIENT_HEADER`] set to that name. An
//! agent side that serves one client (`acp-host`) reads it with
//! [`requesting_client`] and refuses requests that name someone else.
//!
//! The header is a label, not a credential: any NATS client can set it to any
//! value. It catches a second client misconfigured onto a prefix it should not
//! share; keeping clients apart for real takes NATS permissions per prefix.

use async_nats::HeaderMap;
use bytes::Bytes;
use std::sync::{Arc, PoisonError, RwLock};
use tracing::warn;
use trogon_nats::jetstream::{JetStreamGetStream, JetStreamPublisher};
use trogon_nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};

/// The header naming the client that sent a request: the `clientInfo.name`
/// from that client's last `initialize`. Absent when the client sent no
/// `clientInfo`, when the name cannot travel as a header value, and on
/// requests from older bridges.
pub const ACP_CLIENT_HEADER: &str = "X-Acp-Client";

/// The client a request's headers name, if any.
pub fn requesting_client(headers: Option<&HeaderMap>) -> Option<&str> {
    headers?.get(ACP_CLIENT_HEADER).map(|value| value.as_str())
}

/// The name a bridge stamps on what it sends, shared by its NATS and
/// JetStream clients so that one `initialize` updates both.
#[derive(Debug, Clone, Default)]
pub(crate) struct ClientIdentity(Arc<RwLock<Option<String>>>);

impl ClientIdentity {
    /// Records the client named by an `initialize`; `None` clears it.
    pub(crate) fn set(&self, name: Option<&str>) {
        let name = name.and_then(|name| {
            if is_header_safe(name) {
                Some(name.to_string())
            } else {
                warn!(
                    client = name,
                    "clientInfo.name cannot be sent as a NATS header value; requests go unlabelled"
                );
                None
            }
        });
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = name;
    }

    fn stamp(&self, mut headers: HeaderMap) -> HeaderMap {
        if let Some(name) = self.0.read().unwrap_or_else(PoisonError::into_inner).as_deref() {
            headers.insert(ACP_CLIENT_HEADER, name);
        }
        headers
    }
}

/// A header value must not break the NATS protocol line (no control
/// characters), and NATS trims surrounding whitespace, which would make the
/// name the agent reads differ from the one in `clientInfo`.
fn is_header_safe(name: &str) -> bool {
    !name.is_empty() && name.trim() == name && !name.chars().any(char::is_control)
}

/// A NATS or JetStream client that adds the [`ACP_CLIENT_HEADER`] to every
/// message it publishes or requests, and otherwise behaves as `inner`.
#[derive(Debug, Clone)]
pub(crate) struct ClientStamped<T> {
    inner: T,
    identity: ClientIdentity,
}

impl<T> ClientStamped<T> {
    pub(crate) fn new(inner: T, identity: ClientIdentity) -> Self {
        Self { inner, identity }
    }
}

impl<T: RequestClient> RequestClient for ClientStamped<T> {
    type RequestError = T::RequestError;

    fn request_with_headers<S: async_nats::subject::ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> impl Future<Output = Result<async_nats::Message, Self::RequestError>> + Send {
        self.inner
            .request_with_headers(subject, self.identity.stamp(headers), payload)
    }
}

impl<T: PublishClient> PublishClient for ClientStamped<T> {
    type PublishError = T::PublishError;

    fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> impl Future<Output = Result<(), Self::PublishError>> + Send {
        self.inner
            .publish_with_headers(subject, self.identity.stamp(headers), payload)
    }
}

impl<T: FlushClient> FlushClient for ClientStamped<T> {
    type FlushError = T::FlushError;

    fn flush(&self) -> impl Future<Output = Result<(), Self::FlushError>> + Send {
        self.inner.flush()
    }
}

impl<T: SubscribeClient> SubscribeClient for ClientStamped<T> {
    type SubscribeError = T::SubscribeError;
    type Subscription = T::Subscription;

    fn subscribe<S: async_nats::subject::ToSubject + Send>(
        &self,
        subject: S,
    ) -> impl Future<Output = Result<Self::Subscription, Self::SubscribeError>> + Send {
        self.inner.subscribe(subject)
    }
}

impl<T: JetStreamPublisher> JetStreamPublisher for ClientStamped<T> {
    type PublishError = T::PublishError;
    type AckFuture = T::AckFuture;

    fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> impl Future<Output = Result<Self::AckFuture, Self::PublishError>> + Send {
        self.inner
            .publish_with_headers(subject, self.identity.stamp(headers), payload)
    }
}

impl<T: JetStreamGetStream> JetStreamGetStream for ClientStamped<T> {
    type Error = T::Error;
    type Stream = T::Stream;

    fn get_stream<N: AsRef<str> + Send>(
        &self,
        stream_name: N,
    ) -> impl Future<Output = Result<Self::Stream, Self::Error>> + Send {
        self.inner.get_stream(stream_name)
    }
}

#[cfg(test)]
mod tests;
